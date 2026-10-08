use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

/// Cloud Code's `parameters` uses a protobuf-shaped schema, while its Claude
/// backend validates the translated `tools.*.custom.input_schema` as JSON Schema
/// draft 2020-12. Keep this lowering at the Antigravity boundary: it preserves the
/// common subset and converts protobuf JSON int64 strings back to JSON numbers.
/// Callers must still validate tool arguments against their original schema.
pub(super) fn normalize_tool_parameters(
    parameters: &mut Value,
    budget: &mut SchemaBudget,
) -> Result<(), ()> {
    let lowered = lower_schema(parameters, parameters, &mut BTreeSet::new(), 0, budget);
    if budget.exhausted {
        return Err(());
    }
    *parameters = lowered;
    Ok(())
}

/// Cloud Code accepts these unions but its Claude bridge rejects typed anyOf
/// branches (verified with the real fabric_exec schema). Do not apply this to
/// Gemini models. Fold string literal alternatives exactly; otherwise relax the
/// union and retain its constraints as guidance, never choose an arbitrary branch.
/// The caller must validate generated arguments against the original schema.
pub(super) fn normalize_claude_unions(
    schema: &mut Value,
    budget: &mut SchemaBudget,
) -> Result<(), ()> {
    lower_claude_unions(schema, budget)
}

fn lower_claude_unions(value: &mut Value, budget: &mut SchemaBudget) -> Result<(), ()> {
    let Some(schema) = value.as_object_mut() else {
        return Ok(());
    };
    if let Some(Value::Array(branches)) = schema.remove("anyOf") {
        // Only intersect an existing sibling enum when both sides are strings;
        // an empty intersection cannot be represented by protobuf enum (omitted).
        let literals = string_union_literals(&branches);
        let folded = literals.and_then(|mut literals| {
            if let Some(Value::Array(existing)) = schema.get("enum") {
                literals.retain(|literal| existing.contains(&Value::String(literal.clone())));
            }
            (!literals.is_empty()).then_some(literals)
        });
        if let Some(literals) = folded {
            schema.entry("type").or_insert(json!("string"));
            schema.insert("enum".into(), json!(literals));
        } else {
            let guidance = serde_json::to_string(&branches).expect("JSON value serializes");
            let description = schema.entry("description").or_insert(json!(""));
            let original = description.as_str().unwrap_or_default();
            *description = json!(format!("{original}\nAccepted alternatives (validate against the original tool schema): {guidance}").trim());
            if !budget.charge(description) {
                return Err(());
            }
        }
    }
    // Traverse schema positions only: descriptions/defaults/examples and property
    // names such as `anyOf` are data, not keywords to rewrite.
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        for child in properties.values_mut() {
            lower_claude_unions(child, budget)?;
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = schema.get_mut(key) {
            lower_claude_unions(child, budget)?;
        }
    }
    Ok(())
}

fn string_union_literals(branches: &[Value]) -> Option<BTreeSet<String>> {
    if branches.is_empty() {
        return None;
    }
    let mut literals = BTreeSet::new();
    for branch in branches {
        let branch = branch.as_object()?;
        if branch
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "enum" | "description" | "title"))
            || branch.get("type").is_some_and(|ty| ty != "string")
        {
            return None;
        }
        let values = branch.get("enum")?.as_array()?;
        if values.is_empty() {
            return None;
        }
        for literal in values {
            literals.insert(literal.as_str()?.to_owned());
        }
    }
    Some(literals)
}

/// Shared across all tool schemas in one request. Counting serialized input at
/// every expansion conservatively bounds cloning work, including literal data
/// and repeated acyclic references, without allocating serialized copies.
pub(super) struct SchemaBudget {
    nodes_left: usize,
    bytes_left: usize,
    exhausted: bool,
}

impl Default for SchemaBudget {
    fn default() -> Self {
        Self {
            nodes_left: 4096,
            bytes_left: 1024 * 1024,
            exhausted: false,
        }
    }
}

impl SchemaBudget {
    fn charge(&mut self, value: &Value) -> bool {
        if self.exhausted || self.nodes_left == 0 {
            self.exhausted = true;
            return false;
        }
        self.nodes_left -= 1;
        if serde_json::to_writer(&mut *self, value).is_err() {
            self.exhausted = true;
            return false;
        }
        true
    }
}

impl std::io::Write for SchemaBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.bytes_left {
            return Err(std::io::Error::other(
                "tool schema expansion budget exceeded",
            ));
        }
        self.bytes_left -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// JSON Schema $ref siblings are conjunctive, not an object-spread override.
/// Merge properties and required sets; retain the referenced constraint when
/// two validation keywords cannot be intersected in the wire subset. This
/// relaxes validation rather than manufacturing a contradictory schema.
fn merge_ref_siblings(mut target: Value, siblings: Value) -> Value {
    let target_object = target.as_object_mut().expect("lowered schema is an object");
    let Value::Object(siblings) = siblings else {
        unreachable!("lowered schema is an object")
    };
    for (key, value) in siblings {
        match (key.as_str(), target_object.get_mut(&key), value) {
            ("properties", Some(Value::Object(properties)), Value::Object(children)) => {
                for (name, child) in children {
                    if let Some(existing) = properties.get_mut(&name) {
                        *existing = merge_ref_siblings(existing.take(), child);
                    } else {
                        properties.insert(name, child);
                    }
                }
            }
            ("required", Some(Value::Array(required)), Value::Array(names)) => {
                let mut seen: BTreeSet<String> = required
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                for name in names {
                    if seen.insert(name.as_str().expect("required contains strings").to_owned()) {
                        required.push(name);
                    }
                }
            }
            ("title" | "description" | "default" | "example", _, value) | (_, None, value) => {
                target_object.insert(key, value);
            }
            _ => {}
        }
    }
    target
}

fn lower_schema(
    value: &Value,
    root: &Value,
    resolving: &mut BTreeSet<String>,
    depth: usize,
    budget: &mut SchemaBudget,
) -> Value {
    // Bound both recursive references and expansion of deeply nested schemas.
    if !budget.charge(value) || depth >= 64 {
        return json!({});
    }
    let Some(source) = value.as_object() else {
        // Boolean schemas have no protobuf equivalent (false is relaxed).
        return json!({});
    };
    let mut source = source.clone();
    if let Some(Value::String(reference)) = source.remove("$ref") {
        if let Some(target) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
            .filter(|target| target.is_object())
        {
            if resolving.insert(reference.clone()) {
                let target = lower_schema(target, root, resolving, depth + 1, budget);
                resolving.remove(&reference);
                let siblings =
                    lower_schema(&Value::Object(source), root, resolving, depth + 1, budget);
                return merge_ref_siblings(target, siblings);
            }
        }
        // Unresolved, external or cyclic refs retain only their sibling fields.
    }

    let mut schema = Map::new();
    // Keep only a typed JSON-Schema subset. Gemini's protobuf JSON mapping renders
    // int64 constraints as strings, but Claude rejects those at custom.input_schema.
    match source.get("type") {
        Some(Value::String(schema_type)) => {
            if let Some(schema_type) = json_schema_type(schema_type) {
                schema.insert("type".to_string(), Value::String(schema_type.to_string()));
            }
        }
        Some(Value::Array(types)) => {
            schema.insert("type".to_string(), Value::Array(types.clone()));
        }
        _ => {}
    }
    for key in ["format", "title", "description", "pattern"] {
        if let Some(value) = source.get(key).filter(|value| value.is_string()) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    if let Some(value) = source.get("nullable").filter(|value| value.is_boolean()) {
        schema.insert("nullable".to_string(), value.clone());
    }
    if let Some(values) = json_schema_string_array(source.get("enum"), true) {
        schema.insert("enum".to_string(), values);
    }
    for key in ["minimum", "maximum"] {
        if let Some(value) = source.get(key).filter(|value| value.is_number()) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    for key in [
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minProperties",
        "maxProperties",
    ] {
        if let Some(value) = source.get(key).and_then(json_schema_nonnegative_integer) {
            schema.insert(key.to_string(), value);
        }
    }
    if let Some(values) = json_schema_string_array(source.get("required"), false) {
        schema.insert("required".to_string(), values);
    }
    if let Some(values) = json_schema_string_array(source.get("propertyOrdering"), false) {
        schema.insert("propertyOrdering".to_string(), values);
    }
    for key in ["default", "example"] {
        if let Some(value) = source.get(key) {
            schema.insert(key.to_string(), value.clone());
        }
    }

    if let Some(constant) = source.get("const") {
        if constant.is_string() {
            schema.insert("enum".to_string(), json!([constant]));
            schema.entry("type").or_insert(json!("string"));
        } else {
            // The protobuf enum field is repeated string. Never stringify numeric
            // or boolean literals into enums: that changes the argument's type.
            let description = schema.entry("description").or_insert(json!(""));
            let prefix = description.as_str().unwrap_or_default();
            *description = json!(format!("{prefix}\nMust equal: {constant}").trim());
        }
    }
    if let Some(values) = schema.get_mut("enum").and_then(Value::as_array_mut) {
        // Mixed/non-string enums cannot be represented without changing types.
        if !values.iter().all(Value::is_string) {
            schema.remove("enum");
        }
    }

    if let Some(properties) = source.get("properties").and_then(Value::as_object) {
        schema.insert(
            "properties".to_string(),
            Value::Object(
                properties
                    .iter()
                    .map(|(name, child)| {
                        (
                            name.clone(),
                            lower_schema(child, root, resolving, depth + 1, budget),
                        )
                    })
                    .collect(),
            ),
        );
    }
    if let Some(items) = source.get("items") {
        schema.insert(
            "items".to_string(),
            lower_schema(items, root, resolving, depth + 1, budget),
        );
    }
    // oneOf's exclusivity is not supported; anyOf retains the alternatives.
    if let Some(branches) = source
        .get("anyOf")
        .or_else(|| source.get("any_of"))
        .or_else(|| source.get("oneOf"))
        .and_then(Value::as_array)
    {
        if !branches.is_empty() {
            schema.insert(
                "anyOf".to_string(),
                Value::Array(
                    branches
                        .iter()
                        .map(|child| lower_schema(child, root, resolving, depth + 1, budget))
                        .collect(),
                ),
            );
        }
    }

    let patterns = source.get("patternProperties").and_then(Value::as_object);
    let wildcard = patterns
        .filter(|patterns| patterns.len() == 1)
        .and_then(|patterns| {
            patterns
                .iter()
                .next()
                .filter(|(pattern, _)| matches!(pattern.as_str(), ".*" | "^.*$"))
        })
        .map(|(_, child)| child);
    // For a catch-all, JSON Schema's additionalProperties applies to no unmatched
    // keys, so even an explicit `false` must not suppress the dictionary values.
    let additional = wildcard.or_else(|| {
        // Without regex matching, an additional-properties constraint could
        // incorrectly reject keys formerly accepted by a pattern. Relax it too.
        patterns
            .is_none_or(Map::is_empty)
            .then(|| source.get("additionalProperties"))
            .flatten()
    });
    if let Some(additional) = additional {
        if additional.is_boolean() {
            // Dropping non-wildcard patterns must not turn their allowed keys into
            // forbidden additional properties. General regex maps are relaxed.
            if additional != &Value::Bool(false) || patterns.is_none_or(Map::is_empty) {
                schema.insert("additionalProperties".to_string(), additional.clone());
            }
        } else {
            schema.insert(
                "additionalProperties".to_string(),
                lower_schema(additional, root, resolving, depth + 1, budget),
            );
        }
    }

    if let Some(Value::Array(types)) = schema.get("type").cloned() {
        schema.remove("type");
        let nullable = types.iter().any(|value| value.as_str() == Some("null"));
        let types: BTreeSet<_> = types
            .iter()
            .filter_map(Value::as_str)
            .filter_map(json_schema_type)
            .filter(|value| *value != "null")
            .collect();
        if types.len() == 1 {
            schema.insert("type".to_string(), json!(types.first().unwrap()));
        } else if !types.is_empty() {
            if !schema.contains_key("anyOf") {
                let branches = typed_union_branches(&mut schema, &types);
                schema.insert("anyOf".to_string(), branches);
            }
        } else if nullable {
            schema.insert("type".to_string(), json!("null"));
        }
        if nullable && !types.is_empty() {
            schema.insert("nullable".to_string(), Value::Bool(true));
        }
    }
    if let Some(schema_type) = schema.get("type").and_then(Value::as_str) {
        let schema_type = schema_type.to_string();
        strip_other_type_keywords(&mut schema, &schema_type);
    }
    Value::Object(schema)
}

/// JSON Schema applies type-specific keywords only to instances of that type, so
/// on a single-typed node the other types' keywords are dead. Cloud Code rejects
/// them anyway (`items` on a STRING fails "$type == Type.ARRAY").
fn strip_other_type_keywords(schema: &mut Map<String, Value>, schema_type: &str) {
    let own = type_specific_schema_keywords(schema_type);
    for other in ["array", "object", "string", "number"] {
        for key in type_specific_schema_keywords(other) {
            if !own.contains(key) {
                schema.remove(*key);
            }
        }
    }
}

/// Splits a `type: [..]` union into typed anyOf branches. Cloud Code validates
/// type-specific keywords against the node's own type (`items` requires ARRAY),
/// so they move from the untyped parent into the branch they constrain.
fn typed_union_branches(schema: &mut Map<String, Value>, types: &BTreeSet<&str>) -> Value {
    let branches = types
        .iter()
        .map(|ty| {
            let mut branch = Map::new();
            branch.insert("type".to_string(), json!(ty));
            for key in type_specific_schema_keywords(ty) {
                if let Some(value) = schema.get(*key) {
                    branch.insert((*key).to_string(), value.clone());
                }
            }
            Value::Object(branch)
        })
        .collect();
    for ty in types {
        for key in type_specific_schema_keywords(ty) {
            schema.remove(*key);
        }
    }
    Value::Array(branches)
}

fn type_specific_schema_keywords(schema_type: &str) -> &'static [&'static str] {
    match schema_type {
        "array" => &["items", "minItems", "maxItems"],
        "object" => &[
            "properties",
            "required",
            "propertyOrdering",
            "additionalProperties",
            "minProperties",
            "maxProperties",
        ],
        // `enum` constrains every instance type, so it stays where it is.
        "string" => &["pattern", "minLength", "maxLength"],
        "number" | "integer" => &["minimum", "maximum"],
        _ => &[],
    }
}

fn json_schema_type(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "array" => Some("array"),
        "boolean" => Some("boolean"),
        "integer" => Some("integer"),
        "null" => Some("null"),
        "number" => Some("number"),
        "object" => Some("object"),
        "string" => Some("string"),
        _ => None,
    }
}

fn json_schema_nonnegative_integer(value: &Value) -> Option<Value> {
    let value = match value {
        Value::Number(value) => value.as_u64(),
        // protobuf JSON encodes int64 fields as decimal strings.
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    }?;
    Some(Value::from(value))
}

fn json_schema_string_array(value: Option<&Value>, require_non_empty: bool) -> Option<Value> {
    let values = value?.as_array()?;
    let mut seen = BTreeSet::new();
    let values = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .filter(|value| seen.insert(*value))
        .map(|value| Value::String(value.to_string()))
        .collect::<Vec<_>>();
    (!require_non_empty || !values.is_empty()).then_some(Value::Array(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_unions_keep_siblings_literals_and_nested_schema_positions() {
        let union = json!({"anyOf":[{"type":"string"},{"type":"object"}]});
        let mut schema = json!({"type":"object", "default":union, "properties":{
            "anyOf":{"type":"array","items":union},
            "map":{"type":"object","additionalProperties":union},
            "literal":{"enum":["b"],"anyOf":[{"type":"string","enum":["a"]},{"type":"string","enum":["b"]}]},
            "constrained":{"description":"Original","type":"string","minLength":2,"anyOf":[{"type":"string","pattern":"a+"},{"type":"number"}]}
        }});
        normalize_claude_unions(&mut schema, &mut SchemaBudget::default()).unwrap();
        assert_eq!(
            schema["default"], union,
            "literal data must not be rewritten"
        );
        assert!(schema["properties"]["anyOf"]["items"]["description"].is_string());
        assert!(schema["properties"]["map"]["additionalProperties"]["description"].is_string());
        assert_eq!(
            schema["properties"]["literal"],
            json!({"type":"string","enum":["b"]})
        );
        let constrained = &schema["properties"]["constrained"];
        assert_eq!(constrained["minLength"], 2);
        assert_eq!(constrained["type"], "string");
        assert!(constrained["description"]
            .as_str()
            .unwrap()
            .starts_with("Original"));
        assert!(constrained["description"]
            .as_str()
            .unwrap()
            .contains("pattern"));
        let once = schema.clone();
        normalize_claude_unions(&mut schema, &mut SchemaBudget::default()).unwrap();
        assert_eq!(schema, once);
    }

    #[test]
    fn claude_union_guidance_is_charged_to_shared_budget() {
        let mut schema = json!({"anyOf":[{"type":"string"},{"type":"number"}]});
        let mut budget = SchemaBudget {
            nodes_left: 4096,
            bytes_left: 8,
            exhausted: false,
        };
        assert!(normalize_claude_unions(&mut schema, &mut budget).is_err());
        assert!(budget.exhausted);
    }

    #[test]
    fn claude_union_folding_does_not_drop_branch_constraints() {
        let mut schema = json!({"anyOf":[
            {"type":"string","enum":["a"],"minLength":2},
            {"type":"string","enum":["b"]}
        ]});
        normalize_claude_unions(&mut schema, &mut SchemaBudget::default()).unwrap();
        assert!(schema.get("enum").is_none());
        assert!(schema["description"]
            .as_str()
            .unwrap()
            .contains("minLength"));
    }

    fn lowered(mut schema: Value) -> Value {
        normalize_tool_parameters(&mut schema, &mut SchemaBudget::default()).unwrap();
        let once = schema.clone();
        normalize_tool_parameters(&mut schema, &mut SchemaBudget::default()).unwrap();
        assert_eq!(schema, once, "lowering must be idempotent");
        schema
    }

    #[test]
    fn reference_siblings_preserve_properties_and_required_without_contradictions() {
        let result = lowered(json!({
            "$defs": {"Base": {
                "type": "object", "properties": {"a": {"type": "string", "minLength": 1}},
                "required": ["a"], "additionalProperties": false
            }},
            "$ref": "#/$defs/Base",
            "properties": {"a": {"maxLength": 8}, "b": {"type": "string"}},
            "required": ["a"]
        }));
        assert_eq!(
            result,
            json!({
                "type": "object", "properties": {
                    "a": {"type": "string", "minLength": 1, "maxLength": 8},
                    "b": {"type": "string"}
                }, "required": ["a"], "additionalProperties": false
            })
        );
        // A property schema named "additionalProperties" is data, not a keyword.
        let merged = merge_ref_siblings(
            json!({"properties": {"additionalProperties": {"type": "string"}}, "required": ["a"]}),
            json!({"properties": {"b": {"type": "number"}}, "required": ["b", "a"]}),
        );
        assert_eq!(merged["required"], json!(["a", "b"]));
        assert_eq!(
            merged["properties"]["additionalProperties"]["type"],
            "string"
        );
    }

    #[test]
    fn sibling_reference_to_completed_target_is_not_a_cycle() {
        let result = lowered(json!({
            "$defs": {"Base": {"type": "object", "properties": {"mode": {"const": "fast"}}}},
            "$ref": "#/$defs/Base", "properties": {"nested": {"$ref": "#/$defs/Base"}}
        }));
        assert_eq!(
            result["properties"]["nested"]["properties"]["mode"],
            json!({"type": "string", "enum": ["fast"]})
        );
    }

    #[test]
    fn acyclic_branching_references_exhaust_budget_without_mutating_input() {
        let mut schema = json!({"$defs": {"D0": {"type": "string"}}, "$ref": "#/$defs/D24"});
        for index in 1..=24 {
            let reference = format!("#/$defs/D{}", index - 1);
            schema["$defs"][format!("D{index}")] = json!({"type": "object", "properties": {
                "left": {"$ref": reference}, "right": {"$ref": reference}
            }});
        }
        let original = schema.clone();
        let mut budget = SchemaBudget::default();
        assert!(normalize_tool_parameters(&mut schema, &mut budget).is_err());
        assert!(budget.exhausted);
        assert_eq!(schema, original);
    }

    #[test]
    fn limits_nodes_and_literal_bytes_not_only_reference_depth() {
        let mut wide = json!({"type": "object", "properties": {}});
        for index in 0..5000 {
            wide["properties"][format!("p{index}")] = json!({});
        }
        let mut budget = SchemaBudget::default();
        assert!(normalize_tool_parameters(&mut wide, &mut budget).is_err());
        assert_eq!(budget.nodes_left, 0);
        let mut literal = json!({"type": "object", "default": "x".repeat(1024 * 1024)});
        assert!(normalize_tool_parameters(&mut literal, &mut SchemaBudget::default()).is_err());
    }

    #[test]
    fn recursively_lowers_schema_nodes_without_touching_property_names_or_literal_data() {
        let literal = json!({ "const": "data", "patternProperties": { "^.*$": 1 } });
        let result = lowered(json!({
            "$schema": "draft", "$id": "id", "x-custom": true,
            "type": "object", "additionalProperties": false,
            "properties": {
                "const": { "const": "value", "readOnly": true },
                "patternProperties": {
                    "type": "array", "uniqueItems": true,
                    "items": { "oneOf": [{ "const": "a" }, { "const": "b" }] }
                },
                "dictionary": {
                    "type": "object",
                    "additionalProperties": {
                        "any_of": [{ "const": "nested" }], "deprecated": true
                    }
                }
            },
            "default": literal, "example": literal,
            "required": ["const"], "propertyOrdering": ["const", "patternProperties"]
        }));
        assert_eq!(
            result,
            json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "const": { "type": "string", "enum": ["value"] },
                    "patternProperties": {
                        "type": "array",
                        "items": { "anyOf": [
                            { "type": "string", "enum": ["a"] },
                            { "type": "string", "enum": ["b"] }
                        ] }
                    },
                    "dictionary": {
                        "type": "object", "additionalProperties": {
                            "anyOf": [{ "type": "string", "enum": ["nested"] }]
                        }
                    }
                },
                "default": literal, "example": literal,
                "required": ["const"], "propertyOrdering": ["const", "patternProperties"]
            })
        );
    }

    #[test]
    fn catch_all_patterns_preserve_dictionary_values_even_with_additional_properties_false() {
        for pattern in [".*", "^.*$"] {
            for additional in [json!(false), json!(true), json!({ "type": "number" })] {
                let mut schema = json!({ "type": "object", "additionalProperties": additional });
                schema["patternProperties"][pattern] = json!({ "const": "value" });
                assert_eq!(
                    lowered(schema),
                    json!({
                        "type": "object", "additionalProperties": { "type": "string", "enum": ["value"] }
                    })
                );
            }
        }
    }

    #[test]
    fn general_patterns_are_not_mistaken_for_catch_all_dictionaries() {
        for patterns in [
            json!({ "^x-": { "type": "string" } }),
            json!({ "^.*$": { "type": "string" }, "^x-": { "maxLength": 5 } }),
        ] {
            for additional in [json!(false), json!({ "type": "number" })] {
                assert_eq!(
                    lowered(json!({
                        "type": "object", "patternProperties": patterns,
                        "additionalProperties": additional
                    })),
                    json!({ "type": "object" })
                );
            }
        }
        assert_eq!(
            lowered(json!({ "type": "object", "additionalProperties": true })),
            json!({ "type": "object", "additionalProperties": true })
        );
    }

    #[test]
    fn resolves_local_references_with_siblings_and_terminates_cycles() {
        let result = lowered(json!({
            "$defs": {
                "Mode": { "const": "fast", "description": "original" },
                "Node": { "type": "object", "properties": { "next": { "$ref": "#/$defs/Node" } } }
            },
            "type": "object",
            "properties": {
                "mode": { "$ref": "#/$defs/Mode", "description": "override" },
                "again": { "$ref": "#/$defs/Mode" },
                "node": { "$ref": "#/$defs/Node" },
                "missing": { "$ref": "#/$defs/Missing", "type": "string" },
                "external": { "$ref": "https://example.test/schema", "description": "external" }
            }
        }));
        assert_eq!(
            result["properties"]["mode"],
            json!({ "type": "string", "enum": ["fast"], "description": "override" })
        );
        assert_eq!(result["properties"]["again"]["enum"], json!(["fast"]));
        assert_eq!(
            result["properties"]["node"]["properties"]["next"],
            json!({})
        );
        assert_eq!(result["properties"]["missing"], json!({ "type": "string" }));
        assert_eq!(
            result["properties"]["external"],
            json!({ "description": "external" })
        );
        assert!(result.get("$defs").is_none());
    }

    #[test]
    fn lowers_nullable_type_unions_and_non_string_constants_without_invalid_enums() {
        assert_eq!(
            lowered(json!({ "type": ["string", "null", "string"] })),
            json!({ "type": "string", "nullable": true })
        );
        assert_eq!(
            lowered(json!({ "type": ["string", "number"] })),
            json!({ "anyOf": [{ "type": "number" }, { "type": "string" }] })
        );
        for (ty, value) in [
            ("integer", json!(42)),
            ("boolean", json!(true)),
            ("null", Value::Null),
        ] {
            let result = lowered(json!({ "type": ty, "const": value }));
            assert_eq!(result["type"], ty);
            assert_eq!(result["description"], format!("Must equal: {value}"));
            assert!(result.get("enum").is_none());
            assert!(result.get("const").is_none());
        }
        assert_eq!(
            lowered(json!({ "type": "integer", "enum": [1, 2] })),
            json!({ "type": "integer" })
        );
    }

    #[test]
    fn type_unions_move_type_specific_keywords_into_their_branches() {
        // Cloud Code rejected `items` beside an untyped anyOf with
        // "field predicate failed: $type == Type.ARRAY".
        assert_eq!(
            lowered(json!({
                "description": "point or ref",
                "type": ["string", "array"],
                "items": { "type": "number" },
                "minItems": 2,
                "maxLength": 16
            })),
            json!({
                "description": "point or ref",
                "anyOf": [
                    { "type": "array", "items": { "type": "number" }, "minItems": 2 },
                    { "type": "string", "maxLength": 16 }
                ]
            })
        );
        assert_eq!(
            lowered(json!({
                "type": ["object", "integer", "null"],
                "properties": { "a": { "type": "string" } },
                "required": ["a"],
                "minimum": 1
            })),
            json!({
                "nullable": true,
                "anyOf": [
                    { "type": "integer", "minimum": 1 },
                    {
                        "type": "object",
                        "properties": { "a": { "type": "string" } },
                        "required": ["a"]
                    }
                ]
            })
        );
        // An explicit anyOf keeps describing the alternatives untouched.
        assert_eq!(
            lowered(json!({
                "type": ["string", "array"],
                "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "number" } }]
            })),
            json!({
                "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "number" } }]
            })
        );
    }

    #[test]
    fn single_typed_nodes_drop_keywords_of_other_types() {
        // Gemini-native clients may collapse a union to one type but keep `items`
        // (seen from omp): Cloud Code rejects it with "$type == Type.ARRAY".
        assert_eq!(
            lowered(json!({ "type": "string", "items": { "type": "number" } })),
            json!({ "type": "string" })
        );
        assert_eq!(
            lowered(json!({
                "type": "integer",
                "minimum": 1,
                "maxLength": 4,
                "properties": { "a": { "type": "string" } },
                "required": ["a"]
            })),
            json!({ "type": "integer", "minimum": 1 })
        );
        assert_eq!(
            lowered(json!({ "type": "string", "enum": ["a", "b"], "pattern": "^[ab]$" })),
            json!({ "type": "string", "enum": ["a", "b"], "pattern": "^[ab]$" })
        );
    }

    #[test]
    fn converts_protobuf_integer_strings_to_draft_2020_numbers() {
        let result = lowered(json!({
            "type": ["OBJECT", "not-a-json-schema-type"],
            "minProperties": "1",
            "maxProperties": "invalid",
            "required": ["args", "args"],
            "enum": [],
            "anyOf": [],
            "properties": {
                "args": {
                    "type": "ARRAY",
                    "minItems": "2",
                    "maxItems": "3",
                    "minLength": "4",
                    "maxLength": 8,
                    "minimum": "invalid",
                    "maximum": 1,
                    "items": { "type": "STRING", "minLength": "0" }
                }
            }
        }));
        assert_eq!(
            result,
            json!({
                "type": "object",
                "minProperties": 1,
                "required": ["args"],
                "properties": {
                    // String/number keywords are dead on an array node and dropped.
                    "args": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 3,
                        "items": { "type": "string", "minLength": 0 }
                    }
                }
            })
        );
    }

    #[test]
    fn handles_boolean_and_deep_schemas_without_panicking() {
        for input in [json!(true), json!(false), Value::Null] {
            assert_eq!(lowered(input), json!({}));
        }
        let mut nested = json!({ "const": "deep" });
        for _ in 0..70 {
            nested = json!({ "type": "array", "items": nested });
        }
        let result = lowered(nested);
        let mut child = &result;
        for _ in 0..64 {
            assert_eq!(child["type"], "array");
            child = &child["items"];
        }
        assert_eq!(child, &json!({}));
    }
}
