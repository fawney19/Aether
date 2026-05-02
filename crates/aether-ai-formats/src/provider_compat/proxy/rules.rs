use std::{
    collections::{BTreeMap, HashSet},
    sync::OnceLock,
};

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

const ORIGINAL_PLACEHOLDER: &str = "{{$original}}";
const ITEM_PREFIX: &str = "$item.";
const ITEM_EXACT: &str = "$item";
const CONDITION_SOURCES: &[&str] = &["current", "original"];
const CONDITION_TYPE_VALUES: &[&str] = &["string", "number", "boolean", "array", "object", "null"];
const NAME_STYLE_VALUES: &[&str] = &[
    "snake_case",
    "camelCase",
    "PascalCase",
    "kebab-case",
    "capitalize",
];

static RANGE_RE: OnceLock<Regex> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyPathSegment {
    Key(String),
    Index(isize),
    WildcardSlice(Option<usize>, Option<usize>),
}

pub fn header_rules_are_locally_supported(rules: Option<&Value>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    rules.is_array()
}

pub fn apply_local_header_rules(
    headers: &mut BTreeMap<String, String>,
    rules: Option<&Value>,
    protected_keys: &[&str],
    body: &Value,
    original_body: Option<&Value>,
) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let Some(rules) = rules.as_array() else {
        return false;
    };
    let protected_keys: HashSet<String> = protected_keys
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect();

    for rule in rules {
        let Some(rule) = rule.as_object() else {
            continue;
        };
        if let Some(condition) = rule.get("condition").filter(|value| !value.is_null()) {
            if !condition_is_locally_supported(condition) {
                continue;
            }
            if !evaluate_local_condition(body, condition, original_body) {
                continue;
            }
        }

        match rule
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("set") => {
                let Some(key) = rule.get("key").and_then(Value::as_str).map(str::trim) else {
                    continue;
                };
                let key = key.to_ascii_lowercase();
                if key.is_empty() || protected_keys.contains(&key) {
                    continue;
                }
                let value = rule
                    .get("value")
                    .map(header_rule_value_to_string)
                    .unwrap_or_default();
                headers.insert(key, value);
            }
            Some("drop") => {
                let Some(key) = rule.get("key").and_then(Value::as_str).map(str::trim) else {
                    continue;
                };
                let key = key.to_ascii_lowercase();
                if !key.is_empty() && !protected_keys.contains(&key) {
                    headers.remove(&key);
                }
            }
            Some("rename") => {
                let Some(from) = rule.get("from").and_then(Value::as_str).map(str::trim) else {
                    continue;
                };
                let Some(to) = rule.get("to").and_then(Value::as_str).map(str::trim) else {
                    continue;
                };
                let from = from.to_ascii_lowercase();
                let to = to.to_ascii_lowercase();
                if from.is_empty()
                    || to.is_empty()
                    || protected_keys.contains(&from)
                    || protected_keys.contains(&to)
                {
                    continue;
                }
                if let Some(value) = headers.remove(&from) {
                    headers.insert(to, value);
                }
            }
            _ => continue,
        }
    }

    true
}

fn header_rule_value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

pub fn body_rules_are_locally_supported(rules: Option<&Value>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    rules.is_array()
}

pub fn body_rules_handle_path(rules: Option<&Value>, path: &str) -> bool {
    let Some(target_path) = parse_body_path(path) else {
        return false;
    };
    let Some(rules) = rules.and_then(Value::as_array) else {
        return false;
    };

    rules.iter().any(|rule| {
        let Some(rule) = rule.as_object() else {
            return false;
        };
        match rule
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("set")
            | Some("drop")
            | Some("append")
            | Some("insert")
            | Some("regex_replace")
            | Some("name_style") => rule
                .get("path")
                .and_then(Value::as_str)
                .and_then(parse_body_path)
                .is_some_and(|candidate| path_matches(&candidate, &target_path)),
            Some("rename") => {
                rule.get("from")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                    .is_some_and(|candidate| path_matches(&candidate, &target_path))
                    || rule
                        .get("to")
                        .and_then(Value::as_str)
                        .and_then(parse_body_path)
                        .is_some_and(|candidate| path_matches(&candidate, &target_path))
            }
            _ => false,
        }
    })
}

pub fn apply_local_body_rules(
    body: &mut Value,
    rules: Option<&Value>,
    original_body: Option<&Value>,
) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let Some(rules) = rules.as_array() else {
        return false;
    };

    for rule in rules {
        let Some(rule) = rule.as_object() else {
            continue;
        };

        let condition = rule.get("condition").filter(|value| !value.is_null());
        let item_condition = condition.is_some_and(condition_has_item_ref);
        if let Some(condition) = condition {
            if !condition_is_locally_supported(condition) {
                continue;
            }
            if !item_condition && !evaluate_local_condition(body, condition, original_body) {
                continue;
            }
        }

        match rule
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("set") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let targets = iter_wildcard_targets(
                    body,
                    &path,
                    condition,
                    item_condition,
                    original_body,
                    false,
                    false,
                );
                let value_template = rule.get("value").cloned().unwrap_or(Value::Null);
                for target_path in targets {
                    let value = if contains_original_placeholder(&value_template) {
                        let original = get_nested_value(body, &target_path);
                        resolve_original_placeholder(&value_template, original.as_ref())
                    } else {
                        value_template.clone()
                    };
                    let _ = set_nested_value(body, &target_path, value);
                }
            }
            Some("drop") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                for target_path in iter_wildcard_targets(
                    body,
                    &path,
                    condition,
                    item_condition,
                    original_body,
                    true,
                    true,
                ) {
                    let _ = delete_nested_value(body, &target_path);
                }
            }
            Some("rename") => {
                let Some(from) = rule
                    .get("from")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let Some(to) = rule
                    .get("to")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                if has_wildcard(&from) || has_wildcard(&to) {
                    continue;
                }
                let _ = rename_nested_value(body, &from, &to);
            }
            Some("append") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let value = rule.get("value").cloned().unwrap_or(Value::Null);
                for target_path in iter_wildcard_targets(
                    body,
                    &path,
                    condition,
                    item_condition,
                    original_body,
                    true,
                    false,
                ) {
                    if let Some(target) = get_nested_value_mut(body, &target_path) {
                        if let Some(values) = target.as_array_mut() {
                            values.push(value.clone());
                        }
                    }
                }
            }
            Some("insert") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let Some(index) = rule.get("index").and_then(parse_insert_index) else {
                    continue;
                };
                if has_wildcard(&path) {
                    continue;
                }
                let value = rule.get("value").cloned().unwrap_or(Value::Null);
                if let Some(target) = get_nested_value_mut(body, &path) {
                    if let Some(values) = target.as_array_mut() {
                        let insert_index = normalize_insert_index(values.len(), index);
                        values.insert(insert_index, value);
                    }
                }
            }
            Some("regex_replace") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let Some(pattern) = rule.get("pattern").and_then(Value::as_str) else {
                    continue;
                };
                let Some(replacement) = rule.get("replacement").and_then(Value::as_str) else {
                    continue;
                };
                let flags = rule.get("flags").and_then(Value::as_str).unwrap_or("");
                let count = rule
                    .get("count")
                    .and_then(parse_non_negative_count)
                    .unwrap_or(0);
                if pattern.is_empty() {
                    continue;
                }
                let Some(pattern) = compile_regex(pattern, flags) else {
                    continue;
                };
                for target_path in iter_wildcard_targets(
                    body,
                    &path,
                    condition,
                    item_condition,
                    original_body,
                    true,
                    false,
                ) {
                    if let Some(target) = get_nested_value_mut(body, &target_path) {
                        let Some(current) = target.as_str().map(str::to_owned) else {
                            continue;
                        };
                        let replaced = if count == 0 {
                            pattern.replace_all(&current, replacement).to_string()
                        } else {
                            pattern.replacen(&current, count, replacement).to_string()
                        };
                        *target = Value::String(replaced);
                    }
                }
            }
            Some("name_style") => {
                let Some(path) = rule
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(parse_body_path)
                else {
                    continue;
                };
                let Some(style) = rule.get("style").and_then(Value::as_str) else {
                    continue;
                };
                if !valid_name_style(style) {
                    continue;
                }
                for target_path in iter_wildcard_targets(
                    body,
                    &path,
                    condition,
                    item_condition,
                    original_body,
                    true,
                    false,
                ) {
                    if let Some(target) = get_nested_value_mut(body, &target_path) {
                        let Some(current) = target.as_str().map(str::to_owned) else {
                            continue;
                        };
                        *target = Value::String(convert_name_style(&current, style));
                    }
                }
            }
            _ => continue,
        }
    }

    true
}

fn condition_is_locally_supported(condition: &Value) -> bool {
    let Some(condition) = condition.as_object() else {
        return false;
    };

    if let Some(children) = condition.get("all").and_then(Value::as_array) {
        return !children.is_empty() && children.iter().all(condition_is_locally_supported);
    }
    if let Some(children) = condition.get("any").and_then(Value::as_array) {
        return !children.is_empty() && children.iter().all(condition_is_locally_supported);
    }

    let source = condition
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("current");
    if !CONDITION_SOURCES.contains(&source) {
        return false;
    }

    let Some(op) = condition.get("op").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    let Some(path) = condition
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .and_then(parse_body_path)
    else {
        return false;
    };
    if path.is_empty() {
        return false;
    }

    match op {
        "exists" | "not_exists" | "eq" | "neq" => true,
        "gt" | "lt" | "gte" | "lte" => condition
            .get("value")
            .is_some_and(|value| value.as_f64().is_some() && !value.is_boolean()),
        "starts_with" | "ends_with" | "matches" => condition
            .get("value")
            .and_then(Value::as_str)
            .is_some_and(|value| {
                if op == "matches" {
                    Regex::new(value).is_ok()
                } else {
                    true
                }
            }),
        "contains" => condition.get("value").is_some(),
        "in" => condition.get("value").is_some_and(Value::is_array),
        "type_is" => condition
            .get("value")
            .and_then(Value::as_str)
            .is_some_and(|value| CONDITION_TYPE_VALUES.contains(&value)),
        _ => false,
    }
}

fn evaluate_local_condition(
    body: &Value,
    condition: &Value,
    original_body: Option<&Value>,
) -> bool {
    let Some(condition) = condition.as_object() else {
        return false;
    };

    if let Some(children) = condition.get("all").and_then(Value::as_array) {
        return !children.is_empty()
            && children
                .iter()
                .all(|child| evaluate_local_condition(body, child, original_body));
    }
    if let Some(children) = condition.get("any").and_then(Value::as_array) {
        return !children.is_empty()
            && children
                .iter()
                .any(|child| evaluate_local_condition(body, child, original_body));
    }

    let source = condition
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("current");
    let target = if source.eq_ignore_ascii_case("original") {
        original_body.unwrap_or(body)
    } else {
        body
    };

    let Some(op) = condition.get("op").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    let Some(path) = condition
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .and_then(parse_body_path)
    else {
        return false;
    };

    let current_value = get_nested_value(target, &path);
    if op == "exists" {
        return current_value.is_some();
    }
    if op == "not_exists" {
        return current_value.is_none();
    }

    let Some(current_value) = current_value else {
        return false;
    };
    let expected = condition.get("value");

    match op {
        "eq" => expected == Some(&current_value),
        "neq" => expected != Some(&current_value),
        "gt" | "lt" | "gte" | "lte" => {
            let Some(current) = json_number(&current_value) else {
                return false;
            };
            let Some(expected) = expected.and_then(json_number) else {
                return false;
            };
            match op {
                "gt" => current > expected,
                "lt" => current < expected,
                "gte" => current >= expected,
                "lte" => current <= expected,
                _ => false,
            }
        }
        "starts_with" => current_value
            .as_str()
            .zip(expected.and_then(Value::as_str))
            .is_some_and(|(current, expected)| current.starts_with(expected)),
        "ends_with" => current_value
            .as_str()
            .zip(expected.and_then(Value::as_str))
            .is_some_and(|(current, expected)| current.ends_with(expected)),
        "contains" => match (current_value, expected) {
            (Value::String(current), Some(Value::String(expected))) => current.contains(expected),
            (Value::Array(current), Some(expected)) => {
                current.iter().any(|value| value == expected)
            }
            _ => false,
        },
        "matches" => current_value
            .as_str()
            .zip(expected.and_then(Value::as_str))
            .is_some_and(|(current, expected)| {
                Regex::new(expected)
                    .map(|pattern| pattern.is_match(current))
                    .unwrap_or(false)
            }),
        "in" => expected
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(|value| value == &current_value)),
        "type_is" => expected
            .and_then(Value::as_str)
            .is_some_and(|expected| match expected {
                "string" => current_value.is_string(),
                "number" => current_value.as_f64().is_some() && !current_value.is_boolean(),
                "boolean" => current_value.is_boolean(),
                "array" => current_value.is_array(),
                "object" => current_value.is_object(),
                "null" => current_value.is_null(),
                _ => false,
            }),
        _ => false,
    }
}

fn json_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|_| !value.is_boolean())
}

fn parse_body_path(path: &str) -> Option<Vec<BodyPathSegment>> {
    let raw = path.trim();
    if raw.is_empty() {
        return None;
    }

    let chars: Vec<char> = raw.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut expect_key = true;
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' && chars.get(index + 1).copied() == Some('.') {
            current.push('.');
            expect_key = false;
            index += 2;
            continue;
        }

        if ch == '.' {
            if !current.is_empty() {
                parts.push(BodyPathSegment::Key(std::mem::take(&mut current)));
            } else if expect_key {
                return None;
            }
            expect_key = true;
            index += 1;
            continue;
        }

        if ch == '[' {
            if !current.is_empty() {
                parts.push(BodyPathSegment::Key(std::mem::take(&mut current)));
            }

            let mut close_index = index + 1;
            while close_index < chars.len() && chars[close_index] != ']' {
                close_index += 1;
            }
            if close_index >= chars.len() {
                return None;
            }

            let inner = chars[index + 1..close_index]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            if inner.is_empty() {
                return None;
            }

            if inner == "*" {
                parts.push(BodyPathSegment::WildcardSlice(None, None));
            } else if let Some((start, end)) = parse_path_range(&inner) {
                parts.push(BodyPathSegment::WildcardSlice(Some(start), Some(end)));
            } else {
                let Ok(index_value) = inner.parse::<isize>() else {
                    return None;
                };
                parts.push(BodyPathSegment::Index(index_value));
            }

            expect_key = false;
            index = close_index + 1;
            continue;
        }

        current.push(ch);
        expect_key = false;
        index += 1;
    }

    if !current.is_empty() {
        parts.push(BodyPathSegment::Key(current));
    } else if expect_key {
        return None;
    }

    (!parts.is_empty()).then_some(parts)
}

fn parse_path_range(raw: &str) -> Option<(usize, usize)> {
    let captures = range_regex().captures(raw)?;
    let start = captures.get(1)?.as_str().parse::<usize>().ok()?;
    let end = captures.get(2)?.as_str().parse::<usize>().ok()?;
    Some((start, end))
}

fn range_regex() -> &'static Regex {
    RANGE_RE.get_or_init(|| Regex::new(r"^(\d+)\s*-\s*(\d+)$").expect("valid range regex"))
}

fn has_wildcard(path: &[BodyPathSegment]) -> bool {
    path.iter()
        .any(|segment| matches!(segment, BodyPathSegment::WildcardSlice(..)))
}

fn wildcard_indices(
    len: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Box<dyn Iterator<Item = usize>> {
    if len == 0 {
        return Box::new(std::iter::empty());
    }
    match (start, end) {
        (None, None) => Box::new(0..len),
        (Some(start), Some(end)) => {
            let start = start.min(len);
            let end = end.min(len.saturating_sub(1));
            if start > end {
                Box::new(std::iter::empty())
            } else {
                Box::new(start..(end + 1))
            }
        }
        _ => Box::new(std::iter::empty()),
    }
}

fn wildcard_matches_index(start: Option<usize>, end: Option<usize>, index: isize) -> bool {
    match (start, end) {
        (None, None) => true,
        (Some(start), Some(end)) => usize::try_from(index)
            .ok()
            .is_some_and(|index| index >= start && index <= end),
        _ => false,
    }
}

fn path_matches(candidate: &[BodyPathSegment], target: &[BodyPathSegment]) -> bool {
    candidate.len() == target.len()
        && candidate
            .iter()
            .zip(target)
            .all(|(candidate, target)| match (candidate, target) {
                (BodyPathSegment::Key(lhs), BodyPathSegment::Key(rhs)) => lhs == rhs,
                (BodyPathSegment::Index(lhs), BodyPathSegment::Index(rhs)) => lhs == rhs,
                (BodyPathSegment::WildcardSlice(start, end), BodyPathSegment::Index(index))
                | (BodyPathSegment::Index(index), BodyPathSegment::WildcardSlice(start, end)) => {
                    wildcard_matches_index(*start, *end, *index)
                }
                (
                    BodyPathSegment::WildcardSlice(lhs_start, lhs_end),
                    BodyPathSegment::WildcardSlice(rhs_start, rhs_end),
                ) => lhs_start == rhs_start && lhs_end == rhs_end,
                _ => false,
            })
}

fn segments_to_path(path: &[BodyPathSegment]) -> String {
    let mut parts = Vec::new();
    for segment in path {
        match segment {
            BodyPathSegment::Index(index) => parts.push(format!("[{index}]")),
            BodyPathSegment::WildcardSlice(None, None) => parts.push("[*]".to_string()),
            BodyPathSegment::WildcardSlice(Some(start), Some(end)) => {
                parts.push(format!("[{start}-{end}]"));
            }
            BodyPathSegment::WildcardSlice(..) => return String::new(),
            BodyPathSegment::Key(key) => {
                let escaped = key.replace('.', "\\.");
                if parts
                    .last()
                    .is_some_and(|part: &String| !part.ends_with(']'))
                {
                    parts.push(format!(".{escaped}"));
                } else {
                    parts.push(escaped);
                }
            }
        }
    }
    parts.join("")
}

fn contains_original_placeholder(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains(ORIGINAL_PLACEHOLDER),
        Value::Array(items) => items.iter().any(contains_original_placeholder),
        Value::Object(items) => items.values().any(contains_original_placeholder),
        _ => false,
    }
}

fn resolve_original_placeholder(template: &Value, original: Option<&Value>) -> Value {
    match template {
        Value::String(value) => {
            if value == ORIGINAL_PLACEHOLDER {
                return original.cloned().unwrap_or(Value::Null);
            }
            if value.contains(ORIGINAL_PLACEHOLDER) {
                let replacement = original.map_or_else(|| "null".to_string(), placeholder_string);
                return Value::String(value.replace(ORIGINAL_PLACEHOLDER, &replacement));
            }
            Value::String(value.clone())
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_original_placeholder(item, original))
                .collect(),
        ),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .map(|(key, value)| (key.clone(), resolve_original_placeholder(value, original)))
                .collect(),
        ),
        _ => template.clone(),
    }
}

fn placeholder_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn resolve_index(len: usize, index: isize) -> Option<usize> {
    if index >= 0 {
        ((index as usize) < len).then_some(index as usize)
    } else {
        let resolved = len as isize + index;
        (resolved >= 0).then_some(resolved as usize)
    }
}

fn normalize_insert_index(len: usize, index: isize) -> usize {
    if index >= 0 {
        (index as usize).min(len)
    } else {
        let resolved = len as isize + index;
        resolved.max(0) as usize
    }
}

fn expand_wildcard_paths(
    value: &Value,
    path: &[BodyPathSegment],
    require_leaf: bool,
) -> Vec<Vec<BodyPathSegment>> {
    let mut result = Vec::new();
    let mut prefix = Vec::new();
    expand_wildcard_paths_recursive(value, path, require_leaf, 0, &mut prefix, &mut result);
    result
}

fn expand_wildcard_paths_recursive(
    current: &Value,
    path: &[BodyPathSegment],
    require_leaf: bool,
    offset: usize,
    prefix: &mut Vec<BodyPathSegment>,
    result: &mut Vec<Vec<BodyPathSegment>>,
) {
    if offset == path.len() {
        result.push(prefix.clone());
        return;
    }

    let segment = &path[offset];
    let is_last = offset == path.len() - 1;
    match segment {
        BodyPathSegment::WildcardSlice(start, end) => {
            let Some(values) = current.as_array() else {
                return;
            };
            for index in wildcard_indices(values.len(), *start, *end) {
                prefix.push(BodyPathSegment::Index(index as isize));
                expand_wildcard_paths_recursive(
                    &values[index],
                    path,
                    require_leaf,
                    offset + 1,
                    prefix,
                    result,
                );
                prefix.pop();
            }
        }
        BodyPathSegment::Index(index) => {
            let Some(values) = current.as_array() else {
                return;
            };
            let Some(resolved) = resolve_index(values.len(), *index) else {
                return;
            };
            prefix.push(BodyPathSegment::Index(*index));
            expand_wildcard_paths_recursive(
                &values[resolved],
                path,
                require_leaf,
                offset + 1,
                prefix,
                result,
            );
            prefix.pop();
        }
        BodyPathSegment::Key(key) => {
            let Some(object) = current.as_object() else {
                return;
            };
            if let Some(next) = object.get(key) {
                prefix.push(BodyPathSegment::Key(key.clone()));
                expand_wildcard_paths_recursive(
                    next,
                    path,
                    require_leaf,
                    offset + 1,
                    prefix,
                    result,
                );
                prefix.pop();
            } else if is_last && !require_leaf {
                prefix.push(BodyPathSegment::Key(key.clone()));
                result.push(prefix.clone());
                prefix.pop();
            }
        }
    }
}

fn iter_wildcard_targets(
    body: &Value,
    path: &[BodyPathSegment],
    condition: Option<&Value>,
    item_condition: bool,
    original_body: Option<&Value>,
    require_leaf: bool,
    reverse: bool,
) -> Vec<Vec<BodyPathSegment>> {
    if !has_wildcard(path) {
        return vec![path.to_vec()];
    }

    let mut expanded = expand_wildcard_paths(body, path, require_leaf);
    if reverse {
        expanded.reverse();
    }

    if !item_condition {
        return expanded;
    }

    let Some(condition) = condition else {
        return expanded;
    };

    expanded
        .into_iter()
        .filter(|concrete_path| {
            let prefix = get_item_prefix_from_concrete(concrete_path, path);
            let resolved = resolve_item_condition(condition, &prefix);
            evaluate_local_condition(body, &resolved, original_body)
        })
        .collect()
}

fn get_item_prefix_from_concrete(
    concrete_path: &[BodyPathSegment],
    wildcard_path: &[BodyPathSegment],
) -> String {
    let last_wildcard_index = wildcard_path
        .iter()
        .enumerate()
        .rev()
        .find(|(_, segment)| matches!(segment, BodyPathSegment::WildcardSlice(..)))
        .map(|(index, _)| index)
        .unwrap_or(0);
    segments_to_path(&concrete_path[..=last_wildcard_index])
}

fn condition_has_item_ref(condition: &Value) -> bool {
    let Some(condition) = condition.as_object() else {
        return false;
    };
    if let Some(children) = condition.get("all").and_then(Value::as_array) {
        return children.iter().any(condition_has_item_ref);
    }
    if let Some(children) = condition.get("any").and_then(Value::as_array) {
        return children.iter().any(condition_has_item_ref);
    }
    condition
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|path| path == ITEM_EXACT || path.starts_with(ITEM_PREFIX))
}

fn resolve_item_condition(condition: &Value, item_path_prefix: &str) -> Value {
    let Some(condition) = condition.as_object() else {
        return condition.clone();
    };

    if let Some(children) = condition.get("all").and_then(Value::as_array) {
        return serde_json::json!({
            "all": children
                .iter()
                .map(|child| resolve_item_condition(child, item_path_prefix))
                .collect::<Vec<_>>()
        });
    }
    if let Some(children) = condition.get("any").and_then(Value::as_array) {
        return serde_json::json!({
            "any": children
                .iter()
                .map(|child| resolve_item_condition(child, item_path_prefix))
                .collect::<Vec<_>>()
        });
    }

    let mut resolved = condition.clone();
    if let Some(Value::String(path)) = resolved.get_mut("path") {
        let trimmed = path.trim();
        if trimmed == ITEM_EXACT {
            *path = item_path_prefix.to_string();
        } else if let Some(suffix) = trimmed.strip_prefix(ITEM_PREFIX) {
            *path = format!("{item_path_prefix}.{suffix}");
        }
    }
    Value::Object(resolved)
}

fn compile_regex(pattern: &str, flags: &str) -> Option<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    for flag in flags.chars() {
        match flag {
            'i' => {
                builder.case_insensitive(true);
            }
            'm' => {
                builder.multi_line(true);
            }
            's' => {
                builder.dot_matches_new_line(true);
            }
            _ => {}
        }
    }
    builder.build().ok()
}

fn parse_insert_index(value: &Value) -> Option<isize> {
    value
        .as_i64()
        .and_then(|value| isize::try_from(value).ok())
        .or_else(|| value.as_u64().and_then(|value| isize::try_from(value).ok()))
}

fn parse_non_negative_count(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            value.as_i64().and_then(|value| {
                if value < 0 {
                    None
                } else {
                    usize::try_from(value).ok()
                }
            })
        })
}

fn valid_name_style(style: &str) -> bool {
    NAME_STYLE_VALUES.contains(&style)
}

fn convert_name_style(name: &str, style: &str) -> String {
    let words = split_identifier(name);
    if words.is_empty() {
        return name.to_string();
    }

    match style {
        "snake_case" => words.join("_"),
        "camelCase" => {
            let mut result = words[0].clone();
            for word in words.iter().skip(1) {
                result.push_str(&capitalize_ascii(word));
            }
            result
        }
        "PascalCase" => words
            .iter()
            .map(|word| capitalize_ascii(word))
            .collect::<String>(),
        "kebab-case" => words.join("-"),
        "capitalize" => name
            .chars()
            .next()
            .map(|first| {
                let mut result = first.to_uppercase().collect::<String>();
                result.push_str(name.chars().skip(1).collect::<String>().as_str());
                result
            })
            .unwrap_or_default(),
        _ => name.to_string(),
    }
}

fn split_identifier(name: &str) -> Vec<String> {
    let normalized = name.replace(['_', '-'], " ");
    let chars: Vec<char> = normalized.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();

    for (index, ch) in chars.iter().copied().enumerate() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }

        let boundary = if current.is_empty() {
            false
        } else {
            let prev = chars[index - 1];
            let next = chars.get(index + 1).copied();
            (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
                || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                || (prev.is_ascii_uppercase()
                    && ch.is_ascii_uppercase()
                    && next.is_some_and(|next| next.is_ascii_lowercase()))
        };

        if boundary {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
        .into_iter()
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

fn capitalize_ascii(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut result = first.to_uppercase().collect::<String>();
    result.push_str(chars.as_str());
    result
}

fn get_nested_value(value: &Value, path: &[BodyPathSegment]) -> Option<Value> {
    let mut current = value;
    for segment in path {
        match segment {
            BodyPathSegment::Key(key) => {
                current = current.as_object()?.get(key)?;
            }
            BodyPathSegment::Index(index) => {
                let values = current.as_array()?;
                let resolved = resolve_index(values.len(), *index)?;
                current = values.get(resolved)?;
            }
            BodyPathSegment::WildcardSlice(..) => return None,
        }
    }
    Some(current.clone())
}

fn get_nested_value_mut<'a>(
    mut current: &'a mut Value,
    path: &[BodyPathSegment],
) -> Option<&'a mut Value> {
    for segment in path {
        current = get_existing_child_mut(current, segment)?;
    }
    Some(current)
}

fn get_existing_child_mut<'a>(
    current: &'a mut Value,
    segment: &BodyPathSegment,
) -> Option<&'a mut Value> {
    match segment {
        BodyPathSegment::Key(key) => current.as_object_mut()?.get_mut(key),
        BodyPathSegment::Index(index) => {
            let values = current.as_array_mut()?;
            let resolved = resolve_index(values.len(), *index)?;
            values.get_mut(resolved)
        }
        BodyPathSegment::WildcardSlice(..) => None,
    }
}

fn set_nested_value(current: &mut Value, path: &[BodyPathSegment], value: Value) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut current = current;

    for (offset, segment) in parents.iter().enumerate() {
        let next = &path[offset + 1];
        match segment {
            BodyPathSegment::Key(key) => {
                let Some(object) = current.as_object_mut() else {
                    return false;
                };
                match next {
                    BodyPathSegment::Key(_) => {
                        let child = object
                            .entry(key.clone())
                            .or_insert_with(|| Value::Object(Map::new()));
                        if !child.is_object() {
                            *child = Value::Object(Map::new());
                        }
                        current = child;
                    }
                    BodyPathSegment::Index(_) => {
                        let Some(child) = object.get_mut(key) else {
                            return false;
                        };
                        if !child.is_array() {
                            return false;
                        }
                        current = child;
                    }
                    BodyPathSegment::WildcardSlice(..) => return false,
                }
            }
            BodyPathSegment::Index(index) => {
                let Some(values) = current.as_array_mut() else {
                    return false;
                };
                let Some(resolved) = resolve_index(values.len(), *index) else {
                    return false;
                };
                current = &mut values[resolved];
            }
            BodyPathSegment::WildcardSlice(..) => return false,
        }
    }

    match last {
        BodyPathSegment::Key(key) => {
            let Some(object) = current.as_object_mut() else {
                return false;
            };
            object.insert(key.clone(), value);
            true
        }
        BodyPathSegment::Index(index) => {
            let Some(values) = current.as_array_mut() else {
                return false;
            };
            let Some(resolved) = resolve_index(values.len(), *index) else {
                return false;
            };
            values[resolved] = value;
            true
        }
        BodyPathSegment::WildcardSlice(..) => false,
    }
}

fn delete_nested_value(current: &mut Value, path: &[BodyPathSegment]) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut current = current;
    for segment in parents {
        let Some(child) = get_existing_child_mut(current, segment) else {
            return false;
        };
        current = child;
    }

    match last {
        BodyPathSegment::Key(key) => current
            .as_object_mut()
            .and_then(|object| object.remove(key))
            .is_some(),
        BodyPathSegment::Index(index) => {
            let Some(values) = current.as_array_mut() else {
                return false;
            };
            let Some(resolved) = resolve_index(values.len(), *index) else {
                return false;
            };
            values.remove(resolved);
            true
        }
        BodyPathSegment::WildcardSlice(..) => false,
    }
}

fn rename_nested_value(
    current: &mut Value,
    from: &[BodyPathSegment],
    to: &[BodyPathSegment],
) -> bool {
    if from == to {
        return get_nested_value(current, from).is_some();
    }

    let Some(value) = get_nested_value(current, from) else {
        return false;
    };
    if !set_nested_value(current, to, value) {
        return false;
    }
    delete_nested_value(current, from)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_local_body_rules, apply_local_header_rules, body_rules_are_locally_supported,
        body_rules_handle_path, header_rules_are_locally_supported,
    };

    #[test]
    fn header_rules_allow_simple_set_drop_and_rename() {
        let rules = serde_json::json!([
            {"action":"set","key":"x-added","value":"1"},
            {"action":"drop","key":"x-drop"},
            {"action":"rename","from":"x-old","to":"x-new"}
        ]);
        assert!(header_rules_are_locally_supported(Some(&rules)));

        let mut headers = std::collections::BTreeMap::from([
            ("x-drop".to_string(), "drop-me".to_string()),
            ("x-old".to_string(), "old-value".to_string()),
            ("authorization".to_string(), "Bearer keep".to_string()),
        ]);
        assert!(apply_local_header_rules(
            &mut headers,
            Some(&rules),
            &["authorization", "content-type"],
            &serde_json::json!({}),
            None,
        ));
        assert_eq!(headers.get("x-added").map(String::as_str), Some("1"));
        assert!(!headers.contains_key("x-drop"));
        assert_eq!(headers.get("x-new").map(String::as_str), Some("old-value"));
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer keep")
        );
    }

    #[test]
    fn header_rules_allow_simple_conditions() {
        let rules = serde_json::json!([
            {"action":"set","key":"x-added","value":"1","condition":{"path":"metadata.mode","op":"eq","value":"safe"}},
            {"action":"set","key":"x-from-original","value":"1","condition":{"path":"metadata.client","op":"exists","source":"original"}}
        ]);
        assert!(header_rules_are_locally_supported(Some(&rules)));

        let mut headers = std::collections::BTreeMap::new();
        assert!(apply_local_header_rules(
            &mut headers,
            Some(&rules),
            &[],
            &serde_json::json!({"metadata":{"mode":"safe"}}),
            Some(&serde_json::json!({"metadata":{"client":"desktop"}})),
        ));
        assert_eq!(headers.get("x-added").map(String::as_str), Some("1"));
        assert_eq!(
            headers.get("x-from-original").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn body_rules_support_all_runtime_actions() {
        let rules = serde_json::json!([
            {"action":"set","path":"model","value":"wrapped-{{$original}}"},
            {"action":"set","path":"metadata.snapshot","value":"{{$original}}","condition":{"path":"metadata.snapshot","op":"exists"}},
            {"action":"rename","from":"messages[0].content","to":"messages[0].text"},
            {"action":"append","path":"messages","value":{"role":"assistant","content":"done"}},
            {"action":"insert","path":"messages","index":-1,"value":{"role":"system","content":"before-last"}},
            {"action":"regex_replace","path":"tools[*].name","pattern":"tool","replacement":"utility","flags":"i","count":1},
            {"action":"name_style","path":"tools[*].kind","style":"camelCase","condition":{"path":"$item.name","op":"starts_with","value":"Writer"}},
            {"action":"drop","path":"tools[1-2].deprecated"}
        ]);
        assert!(body_rules_are_locally_supported(Some(&rules)));

        let mut body = serde_json::json!({
            "model": "gpt-5",
            "metadata": {
                "snapshot": {
                    "keep": true
                }
            },
            "messages": [
                {"content":"hello"},
                {"content":"world"}
            ],
            "tools": [
                {"name":"WriterTool","kind":"snake_case_name","deprecated":true},
                {"name":"ReadTool","kind":"Pascal Case","deprecated":true},
                {"name":"OtherTool","kind":"kebab-case-value","deprecated":true}
            ]
        });

        assert!(apply_local_body_rules(&mut body, Some(&rules), None));

        assert_eq!(body["model"], "wrapped-gpt-5");
        assert_eq!(
            body["metadata"]["snapshot"],
            serde_json::json!({"keep": true})
        );
        assert_eq!(body["messages"][0]["text"], "hello");
        assert!(body["messages"][0].get("content").is_none());
        assert_eq!(body["messages"][1]["content"], "world");
        assert_eq!(
            body["messages"][2],
            serde_json::json!({"role":"system","content":"before-last"})
        );
        assert_eq!(
            body["messages"][3],
            serde_json::json!({"role":"assistant","content":"done"})
        );
        assert_eq!(body["tools"][0]["name"], "Writerutility");
        assert_eq!(body["tools"][1]["name"], "Readutility");
        assert_eq!(body["tools"][2]["name"], "Otherutility");
        assert_eq!(body["tools"][0]["kind"], "snakeCaseName");
        assert_eq!(body["tools"][1]["kind"], "Pascal Case");
        assert_eq!(body["tools"][2]["kind"], "kebab-case-value");
        assert_eq!(body["tools"][0]["deprecated"], true);
        assert!(body["tools"][1].get("deprecated").is_none());
        assert!(body["tools"][2].get("deprecated").is_none());
    }

    #[test]
    fn body_rules_conditions_cover_all_supported_operators() {
        let rules = serde_json::json!([
            {"action":"set","path":"results.eq","value":true,"condition":{"path":"text","op":"eq","value":"alpha-beta"}},
            {"action":"set","path":"results.neq","value":true,"condition":{"path":"text","op":"neq","value":"other"}},
            {"action":"set","path":"results.gt","value":true,"condition":{"path":"num","op":"gt","value":9}},
            {"action":"set","path":"results.lt","value":true,"condition":{"path":"num","op":"lt","value":11}},
            {"action":"set","path":"results.gte","value":true,"condition":{"path":"num","op":"gte","value":10}},
            {"action":"set","path":"results.lte","value":true,"condition":{"path":"num","op":"lte","value":10}},
            {"action":"set","path":"results.starts_with","value":true,"condition":{"path":"text","op":"starts_with","value":"alpha"}},
            {"action":"set","path":"results.ends_with","value":true,"condition":{"path":"text","op":"ends_with","value":"beta"}},
            {"action":"set","path":"results.contains_string","value":true,"condition":{"path":"text","op":"contains","value":"ha-be"}},
            {"action":"set","path":"results.contains_array","value":true,"condition":{"path":"tags","op":"contains","value":"green"}},
            {"action":"set","path":"results.matches","value":true,"condition":{"path":"text","op":"matches","value":"alpha.*beta"}},
            {"action":"set","path":"results.exists","value":true,"condition":{"path":"profile.name","op":"exists"}},
            {"action":"set","path":"results.not_exists","value":true,"condition":{"path":"profile.age","op":"not_exists"}},
            {"action":"set","path":"results.in","value":true,"condition":{"path":"choice","op":"in","value":["a","b","c"]}},
            {"action":"set","path":"results.type_is","value":true,"condition":{"path":"flag","op":"type_is","value":"boolean"}},
            {"action":"set","path":"results.original","value":true,"condition":{"path":"legacy.present","op":"exists","source":"original"}},
            {"action":"set","path":"results.all","value":true,"condition":{"all":[{"path":"num","op":"gt","value":5},{"path":"text","op":"contains","value":"beta"}]}},
            {"action":"set","path":"results.any","value":true,"condition":{"any":[{"path":"missing","op":"exists"},{"path":"maybe_null","op":"type_is","value":"null"}]}}
        ]);
        assert!(body_rules_are_locally_supported(Some(&rules)));

        let original = serde_json::json!({
            "legacy": {
                "present": 1
            }
        });
        let mut body = serde_json::json!({
            "num": 10,
            "text": "alpha-beta",
            "tags": ["red", "green"],
            "choice": "b",
            "flag": true,
            "maybe_null": null,
            "profile": {
                "name": "Ada"
            }
        });

        assert!(apply_local_body_rules(
            &mut body,
            Some(&rules),
            Some(&original)
        ));

        let expected = serde_json::json!({
            "eq": true,
            "neq": true,
            "gt": true,
            "lt": true,
            "gte": true,
            "lte": true,
            "starts_with": true,
            "ends_with": true,
            "contains_string": true,
            "contains_array": true,
            "matches": true,
            "exists": true,
            "not_exists": true,
            "in": true,
            "type_is": true,
            "original": true,
            "all": true,
            "any": true
        });
        assert_eq!(body["results"], expected);
    }

    #[test]
    fn body_rules_tolerate_invalid_regex_flags_and_negative_count() {
        let invalid_flags = serde_json::json!([
            {"action":"regex_replace","path":"text","pattern":"foo","replacement":"bar","flags":"ix"}
        ]);
        let invalid_count = serde_json::json!([
            {"action":"regex_replace","path":"text","pattern":"foo","replacement":"bar","count":-1}
        ]);
        let mut flags_body = serde_json::json!({"text":"foo"});
        let mut count_body = serde_json::json!({"text":"foo foo"});

        assert!(body_rules_are_locally_supported(Some(&invalid_flags)));
        assert!(body_rules_are_locally_supported(Some(&invalid_count)));
        assert!(apply_local_body_rules(
            &mut flags_body,
            Some(&invalid_flags),
            None
        ));
        assert!(apply_local_body_rules(
            &mut count_body,
            Some(&invalid_count),
            None
        ));
        assert_eq!(flags_body["text"], "bar");
        assert_eq!(count_body["text"], "bar bar");
    }

    #[test]
    fn body_rules_skip_invalid_entries_without_rejecting_whole_body() {
        let rules = serde_json::json!([
            {"action":"set","path":".bad","value":1},
            {"action":"drop","path":"missing."},
            {"action":"regex_replace","path":"text","pattern":"(","replacement":"x"},
            {"op":"remove","path":"/legacy"},
            {"action":"set","path":"ok","value":true}
        ]);
        let mut body = serde_json::json!({
            "text": "keep",
            "legacy": true
        });

        assert!(apply_local_body_rules(&mut body, Some(&rules), None));

        assert_eq!(body["text"], "keep");
        assert_eq!(body["legacy"], true);
        assert_eq!(body["ok"], true);
    }

    #[test]
    fn header_rules_skip_invalid_entries_without_rejecting_whole_headers() {
        let rules = serde_json::json!([
            {"action":"set","key":"","value":"bad"},
            {"action":"set","key":"x-json","value":{"nested":true}},
            {"action":"drop","key":null},
            {"action":"rename","from":"x-missing","to":""},
            {"op":"remove","key":"x-legacy"},
            {"action":"set","key":"x-ok","value":"yes"}
        ]);
        let mut headers = std::collections::BTreeMap::from([(
            "authorization".to_string(),
            "Bearer keep".to_string(),
        )]);

        assert!(header_rules_are_locally_supported(Some(&rules)));
        assert!(apply_local_header_rules(
            &mut headers,
            Some(&rules),
            &["authorization"],
            &serde_json::json!({}),
            None,
        ));

        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer keep")
        );
        assert_eq!(
            headers.get("x-json").map(String::as_str),
            Some("{\"nested\":true}")
        );
        assert_eq!(headers.get("x-ok").map(String::as_str), Some("yes"));
        assert!(!headers.contains_key("x-legacy"));
    }

    #[test]
    fn body_rules_handle_exact_and_wildcard_paths() {
        let rules = serde_json::json!([
            {"action":"append","path":"messages","value":{}},
            {"action":"regex_replace","path":"tools[*].name","pattern":"foo","replacement":"bar"},
            {"action":"name_style","path":"tools[0-2].kind","style":"snake_case"},
            {"action":"rename","from":"metadata.old","to":"metadata.new"}
        ]);

        assert!(body_rules_handle_path(Some(&rules), "messages"));
        assert!(body_rules_handle_path(Some(&rules), "tools[1].name"));
        assert!(body_rules_handle_path(Some(&rules), "tools[2].kind"));
        assert!(body_rules_handle_path(Some(&rules), "metadata.old"));
        assert!(body_rules_handle_path(Some(&rules), "metadata.new"));
        assert!(!body_rules_handle_path(Some(&rules), "tools[3].kind"));
        assert!(!body_rules_handle_path(Some(&rules), "instructions"));
    }
}
