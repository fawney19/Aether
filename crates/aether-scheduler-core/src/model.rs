use std::borrow::Cow;
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
    resolve_requested_global_model_name_with_model_directives(
        rows,
        requested_model_name,
        api_format,
        false,
    )
}

pub fn resolve_requested_global_model_name_with_model_directives(
    rows: &[StoredMinimalCandidateSelectionRow],
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
) -> Option<String> {
    resolve_requested_global_model_name_with_model_directives_and_request_operation(
        rows,
        requested_model_name,
        api_format,
        enable_model_directives,
        None,
    )
}

pub fn resolve_requested_global_model_name_with_model_directives_and_request_operation(
    rows: &[StoredMinimalCandidateSelectionRow],
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
    request_operation: Option<&str>,
) -> Option<String> {
    requested_model_name_candidates(requested_model_name, enable_model_directives).find_map(
        |requested_model_name| {
            let requested_model_name = requested_model_name.as_ref();
            resolve_global_model_name_by(rows, |row| {
                row_has_available_provider_model(row, api_format, request_operation)
                    && row.global_model_name == requested_model_name
            })
            .or_else(|| {
                resolve_global_model_name_by(rows, |row| {
                    row_default_provider_model_name_available(row, api_format, request_operation)
                        && row.model_provider_model_name == requested_model_name
                })
            })
            .or_else(|| {
                resolve_global_model_name_by(rows, |row| {
                    row.model_provider_model_mappings
                        .as_ref()
                        .is_some_and(|mappings| {
                            mappings.iter().any(|mapping| {
                                mapping_scope_matches(mapping, row, api_format, request_operation)
                                    && mapping.name == requested_model_name
                            })
                        })
                })
            })
            .or_else(|| {
                resolve_global_model_name_by(rows, |row| {
                    row_has_available_provider_model(row, api_format, request_operation)
                        && row.global_model_mappings.as_ref().is_some_and(|patterns| {
                            patterns
                                .iter()
                                .any(|pattern| matches_model_mapping(pattern, requested_model_name))
                        })
                })
            })
        },
    )
}

pub fn row_supports_requested_model(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
) -> bool {
    row_supports_requested_model_with_model_directives(row, requested_model_name, api_format, false)
}

pub fn row_supports_requested_model_with_model_directives(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
) -> bool {
    row_supports_requested_model_with_model_directives_and_request_operation(
        row,
        requested_model_name,
        api_format,
        enable_model_directives,
        None,
    )
}

pub fn row_supports_requested_model_with_model_directives_and_request_operation(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
    request_operation: Option<&str>,
) -> bool {
    requested_model_name_candidates(requested_model_name, enable_model_directives).any(
        |requested_model_name| {
            row_supports_requested_model_exact(
                row,
                requested_model_name.as_ref(),
                api_format,
                request_operation,
            )
        },
    )
}

fn row_supports_requested_model_exact(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
    request_operation: Option<&str>,
) -> bool {
    row_has_available_provider_model(row, api_format, request_operation)
        && (row.global_model_name == requested_model_name
            || (row_default_provider_model_name_available(row, api_format, request_operation)
                && row.model_provider_model_name == requested_model_name)
            || row.global_model_mappings.as_ref().is_some_and(|patterns| {
                patterns
                    .iter()
                    .any(|pattern| matches_model_mapping(pattern, requested_model_name))
            }))
        || row
            .model_provider_model_mappings
            .as_ref()
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping_scope_matches(mapping, row, api_format, request_operation)
                        && mapping.name == requested_model_name
                })
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
    resolve_provider_model_name_with_model_directives(row, requested_model_name, api_format, false)
}

pub fn resolve_provider_model_name_with_model_directives(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
) -> Option<(String, Option<String>)> {
    resolve_provider_model_name_with_model_directives_and_request_operation(
        row,
        requested_model_name,
        api_format,
        enable_model_directives,
        None,
    )
}

pub fn resolve_provider_model_name_with_model_directives_and_request_operation(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
    request_operation: Option<&str>,
) -> Option<(String, Option<String>)> {
    let selected_provider_model_name =
        resolve_selected_provider_model_name(row, api_format, request_operation)?;
    let Some(key_allowed_models) = row.key_allowed_models.as_ref() else {
        return Some((selected_provider_model_name, None));
    };
    if key_allowed_models.is_empty() {
        return None;
    }

    for candidate_name in
        requested_model_name_candidates(requested_model_name, enable_model_directives)
    {
        if key_allowed_models
            .iter()
            .any(|value| value == candidate_name.as_ref())
        {
            let matched = (candidate_name.as_ref() != requested_model_name)
                .then(|| candidate_name.into_owned());
            return Some((selected_provider_model_name, matched));
        }
    }

    let mut sorted_allowed_models = key_allowed_models
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    sorted_allowed_models.sort_unstable();

    for &allowed_model in &sorted_allowed_models {
        if row_has_candidate_model_name(row, api_format, request_operation, allowed_model) {
            let allowed_model = allowed_model.to_owned();
            return Some((selected_provider_model_name.clone(), Some(allowed_model)));
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
    resolve_selected_provider_model_name(row, api_format, None)
        .unwrap_or_else(|| row.model_provider_model_name.clone())
}

fn resolve_selected_provider_model_name(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    request_operation: Option<&str>,
) -> Option<String> {
    let Some(mappings) = row.model_provider_model_mappings.as_ref() else {
        return Some(row.model_provider_model_name.clone());
    };

    if let Some(mapping) = mappings
        .iter()
        .filter(|mapping| mapping_scope_matches(mapping, row, api_format, request_operation))
        .min_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| {
                    mapping_operation_scope_rank(right).cmp(&mapping_operation_scope_rank(left))
                })
                .then(left.name.cmp(&right.name))
        })
    {
        return Some(mapping.name.clone());
    }

    row_default_provider_model_name_available(row, api_format, request_operation)
        .then(|| row.model_provider_model_name.clone())
}

pub fn candidate_model_names(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if row_default_provider_model_name_available(row, api_format, None) {
        names.insert(row.model_provider_model_name.clone());
    }
    if let Some(mappings) = row.model_provider_model_mappings.as_ref() {
        for mapping in mappings {
            if mapping_scope_matches(mapping, row, api_format, None) {
                names.insert(mapping.name.clone());
            }
        }
    }
    names
}

fn row_has_available_provider_model(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    request_operation: Option<&str>,
) -> bool {
    resolve_selected_provider_model_name(row, api_format, request_operation).is_some()
}

fn row_default_provider_model_name_available(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    request_operation: Option<&str>,
) -> bool {
    let Some(mappings) = row.model_provider_model_mappings.as_ref() else {
        return true;
    };
    let mut has_explicit_default_mapping = false;
    for mapping in mappings {
        if mapping.name != row.model_provider_model_name {
            continue;
        }
        has_explicit_default_mapping = true;
        if mapping_scope_matches(mapping, row, api_format, request_operation) {
            return true;
        }
    }
    !has_explicit_default_mapping
}

fn mapping_scope_matches(
    mapping: &StoredProviderModelMapping,
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    request_operation: Option<&str>,
) -> bool {
    let api_format_matches_scope = mapping.api_formats.as_ref().is_none_or(|api_formats| {
        api_formats
            .iter()
            .any(|value| api_format_scope_covers(value, api_format))
    });
    if !api_format_matches_scope {
        return false;
    }

    let endpoint_matches_scope = mapping.endpoint_ids.as_ref().is_none_or(|endpoint_ids| {
        endpoint_ids
            .iter()
            .any(|endpoint_id| endpoint_id == &row.endpoint_id)
    });
    if !endpoint_matches_scope {
        return false;
    }

    mapping.operations.as_ref().is_none_or(|operations| {
        request_operation.is_some_and(|request_operation| {
            operations
                .iter()
                .any(|operation| operation.eq_ignore_ascii_case(request_operation))
        })
    })
}

fn mapping_operation_scope_rank(mapping: &StoredProviderModelMapping) -> u8 {
    u8::from(mapping.operations.is_some())
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
    aether_ai_formats::normalize_api_format_alias(value)
}

fn row_has_candidate_model_name(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    request_operation: Option<&str>,
    model_name: &str,
) -> bool {
    (row_default_provider_model_name_available(row, api_format, request_operation)
        && row.model_provider_model_name == model_name)
        || row
            .model_provider_model_mappings
            .as_ref()
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping_scope_matches(mapping, row, api_format, request_operation)
                        && mapping.name == model_name
                })
            })
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format(left) == normalize_api_format(right)
}

fn api_format_scope_covers(allowed: &str, requested: &str) -> bool {
    aether_ai_formats::api_format_permission_covers(allowed, requested)
}

fn requested_model_name_candidates(
    requested_model_name: &str,
    enable_model_directives: bool,
) -> impl Iterator<Item = Cow<'_, str>> {
    let requested_model_name = requested_model_name.trim();
    let mut candidates = Vec::new();
    push_model_name_candidate(&mut candidates, Cow::Borrowed(requested_model_name));
    for alias in requested_model_name_aliases(requested_model_name) {
        push_model_name_candidate(&mut candidates, Cow::Owned(alias));
    }
    if enable_model_directives {
        if let Some(base_model) =
            aether_ai_formats::model_directive_base_model(requested_model_name)
        {
            for alias in requested_model_name_aliases(&base_model) {
                push_model_name_candidate(&mut candidates, Cow::Owned(alias));
            }
            push_model_name_candidate(&mut candidates, Cow::Owned(base_model));
        }
    }
    candidates.into_iter()
}

fn push_model_name_candidate<'a>(candidates: &mut Vec<Cow<'a, str>>, candidate: Cow<'a, str>) {
    if candidate.trim().is_empty() {
        return;
    }
    if candidates
        .iter()
        .any(|existing| existing.as_ref() == candidate.as_ref())
    {
        return;
    }
    candidates.push(candidate);
}

fn requested_model_name_aliases(requested_model_name: &str) -> Vec<String> {
    let requested_model_name = requested_model_name.trim();
    let Some(alias) = windsurf_gpt55_model_alias(requested_model_name) else {
        return Vec::new();
    };
    vec![alias]
}

fn windsurf_gpt55_model_alias(model_name: &str) -> Option<String> {
    let suffix = model_name
        .strip_prefix("gpt-5-5")
        .map(|suffix| format!("gpt-5.5{suffix}"))
        .or_else(|| {
            model_name
                .strip_prefix("gpt-5.5")
                .map(|suffix| format!("gpt-5-5{suffix}"))
        })?;
    (suffix != model_name).then_some(suffix)
}

#[cfg(test)]
mod tests {
    use super::{
        matches_model_mapping, resolve_provider_model_name,
        resolve_provider_model_name_with_model_directives,
        resolve_provider_model_name_with_model_directives_and_request_operation,
        resolve_requested_global_model_name_with_model_directives, row_supports_requested_model,
        row_supports_requested_model_with_model_directives,
    };
    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };

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

    #[test]
    fn regex_allowed_model_replaces_selected_provider_model_name() {
        let mut row = sample_row("gpt-5", "gpt-5-upstream");
        row.key_allowed_models = Some(vec!["gpt-5.4".to_string()]);
        row.global_model_mappings = Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]);
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "gpt-5-canonical-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]);

        let resolved = resolve_provider_model_name(&row, "gpt-5", "openai:chat")
            .expect("regex-matched allowed model should allow the key");

        assert_eq!(resolved.0, "gpt-5.4");
        assert_eq!(resolved.1.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn model_directive_suffix_matches_base_model_as_fallback() {
        let row = sample_row("gpt-5.4", "gpt-5.4-upstream");

        assert!(!row_supports_requested_model(
            &row,
            "gpt-5.4-xhigh",
            "openai:chat"
        ));
        assert!(row_supports_requested_model_with_model_directives(
            &row,
            "gpt-5.4-xhigh",
            "openai:chat",
            true
        ));
        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[row],
                "gpt-5.4-xhigh",
                "openai:chat",
                true
            )
            .as_deref(),
            Some("gpt-5.4")
        );

        let row = sample_row("gpt-5.4", "gpt-5.4-upstream");
        assert!(row_supports_requested_model_with_model_directives(
            &row,
            "gpt-5.4-fast-xhigh",
            "openai:chat",
            true
        ));
        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[row],
                "gpt-5.4-xhigh-fast",
                "openai:chat",
                true
            )
            .as_deref(),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn responses_model_mapping_scope_covers_search_in_one_direction() {
        let mut row = sample_row("search-global", "search-default");
        row.endpoint_api_format = "openai:search".to_string();
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "gpt-5.6-sol".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:responses".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]);

        assert!(row_supports_requested_model(
            &row,
            "gpt-5.6-sol",
            "openai:search"
        ));
        assert_eq!(
            resolve_provider_model_name(&row, "gpt-5.6-sol", "openai:search")
                .map(|resolved| resolved.0),
            Some("gpt-5.6-sol".to_string())
        );

        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "search-only".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:search".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]);
        assert!(!row_supports_requested_model(
            &row,
            "search-only",
            "openai:responses"
        ));
    }

    #[test]
    fn operation_scoped_mapping_overrides_generic_mapping_for_compaction() {
        let mut row = sample_row("gpt-5.6-sol", "gpt-5.6-sol");
        row.endpoint_api_format = "openai:responses".to_string();
        row.model_provider_model_mappings = Some(vec![
            StoredProviderModelMapping {
                name: "gpt-5.6-sol".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
                endpoint_ids: None,
                operations: None,
            },
            StoredProviderModelMapping {
                name: "gpt-5.6-terra".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
                endpoint_ids: None,
                operations: Some(vec!["compact".to_string()]),
            },
        ]);

        assert_eq!(
            resolve_provider_model_name_with_model_directives_and_request_operation(
                &row,
                "gpt-5.6-sol",
                "openai:responses",
                false,
                None,
            )
            .map(|resolved| resolved.0),
            Some("gpt-5.6-sol".to_string())
        );
        assert_eq!(
            resolve_provider_model_name_with_model_directives_and_request_operation(
                &row,
                "gpt-5.6-sol",
                "openai:responses",
                false,
                Some("compact"),
            )
            .map(|resolved| resolved.0),
            Some("gpt-5.6-terra".to_string())
        );
    }

    #[test]
    fn model_directive_suffix_prefers_exact_model_before_base_fallback() {
        let exact = sample_row("gpt-5.4-high", "gpt-5.4-high-upstream");
        let base = sample_row("gpt-5.4", "gpt-5.4-upstream");

        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[base, exact],
                "gpt-5.4-high",
                "openai:chat",
                true
            )
            .as_deref(),
            Some("gpt-5.4-high")
        );
    }

    #[test]
    fn model_directive_base_model_satisfies_key_allowed_models() {
        let mut row = sample_row("gpt-5.4", "gpt-5.4-upstream");
        row.key_allowed_models = Some(vec!["gpt-5.4".to_string()]);

        assert!(resolve_provider_model_name(&row, "gpt-5.4-max", "openai:chat").is_none());
        let resolved = resolve_provider_model_name_with_model_directives(
            &row,
            "gpt-5.4-max",
            "openai:chat",
            true,
        )
        .expect("base model should satisfy key allowed models");

        assert_eq!(resolved.0, "gpt-5.4-upstream");
        assert_eq!(resolved.1.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn windsurf_dashed_gpt55_alias_matches_dotted_model_name() {
        let row = sample_row("gpt-5.5-low", "gpt-5.5-low");

        assert!(row_supports_requested_model(
            &row,
            "gpt-5-5-low",
            "openai:chat"
        ));
        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[row],
                "gpt-5-5-low",
                "openai:chat",
                false,
            )
            .as_deref(),
            Some("gpt-5.5-low")
        );
    }

    #[test]
    fn windsurf_dashed_gpt55_alias_satisfies_key_allowed_models() {
        let mut row = sample_row("gpt-5.5-low", "windsurf-upstream-uid");
        row.key_allowed_models = Some(vec!["gpt-5.5-low".to_string()]);

        let resolved = resolve_provider_model_name(&row, "gpt-5-5-low", "openai:chat")
            .expect("dashed alias should satisfy dotted allowed model");

        assert_eq!(resolved.0, "windsurf-upstream-uid");
        assert_eq!(resolved.1.as_deref(), Some("gpt-5.5-low"));
    }

    #[test]
    fn endpoint_scoped_default_mapping_limits_exact_global_model_match() {
        let mut row = sample_row("deepseek-v4-pro", "deepseek-v4-pro");
        row.endpoint_id = "endpoint-claude".to_string();
        row.endpoint_api_format = "claude:messages".to_string();
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "deepseek-v4-pro".to_string(),
            priority: 1,
            api_formats: None,
            endpoint_ids: Some(vec!["endpoint-openai".to_string()]),
            operations: None,
        }]);

        assert!(!row_supports_requested_model(
            &row,
            "deepseek-v4-pro",
            "claude:messages"
        ));
        assert!(resolve_provider_model_name(&row, "deepseek-v4-pro", "claude:messages").is_none());
        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[row.clone()],
                "deepseek-v4-pro",
                "claude:messages",
                false,
            ),
            None
        );

        row.endpoint_id = "endpoint-openai".to_string();
        row.endpoint_api_format = "openai:chat".to_string();

        assert!(row_supports_requested_model(
            &row,
            "deepseek-v4-pro",
            "openai:chat"
        ));
        assert_eq!(
            resolve_requested_global_model_name_with_model_directives(
                &[row],
                "deepseek-v4-pro",
                "openai:chat",
                false,
            )
            .as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    fn sample_row(
        global_model_name: &str,
        model_provider_model_name: &str,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "Provider".to_string(),
            provider_type: "openai".to_string(),
            provider_priority: 0,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: None,
            endpoint_kind: None,
            endpoint_is_active: true,
            key_id: "key-1".to_string(),
            key_name: "Key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: None,
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 0,
            key_global_priority_by_format: None,
            model_id: format!("model-{global_model_name}"),
            global_model_id: format!("global-{global_model_name}"),
            global_model_name: global_model_name.to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: model_provider_model_name.to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }
}
