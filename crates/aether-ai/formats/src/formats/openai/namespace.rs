use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::protocol::canonical::{
    openai_responses_tools_to_canonical, CanonicalToolDefinition,
    OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
};

const OPENAI_CHAT_TOOL_NAME_MAX_LEN: usize = 64;
const HASHED_ALIAS_PREFIX: &str = "aether_ns_";
const HASH_HEX_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamespaceChatTool {
    pub source_tool_index: usize,
    pub source_child_index: usize,
    pub namespace: String,
    pub name: String,
    pub chat_name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
    pub strict: Option<Value>,
}

impl NamespaceChatTool {
    pub(crate) fn to_openai_chat_tool(&self) -> Value {
        let mut function = Map::new();
        function.insert("name".to_string(), Value::String(self.chat_name.clone()));
        if let Some(description) = &self.description {
            function.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(parameters) = &self.parameters {
            function.insert("parameters".to_string(), parameters.clone());
        }
        if let Some(strict) = &self.strict {
            function.insert("strict".to_string(), strict.clone());
        }
        json!({
            "type": "function",
            "function": Value::Object(function),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NamespaceToolAliases {
    tools: Vec<NamespaceChatTool>,
    by_identity: BTreeMap<(String, String), String>,
    by_chat_name: BTreeMap<String, (String, String)>,
    namespace_tool_indices: BTreeSet<usize>,
    invalid_namespace_tool_indices: BTreeSet<usize>,
    client_function_tool_names: BTreeSet<String>,
}

impl NamespaceToolAliases {
    pub(crate) fn from_canonical_tools(tools: &[CanonicalToolDefinition]) -> Self {
        let mut result = Self::default();
        let mut parsed = Vec::new();
        let mut name_counts = BTreeMap::<String, usize>::new();
        let mut ordinary_chat_names = BTreeSet::<String>::new();

        for (tool_index, tool) in tools.iter().enumerate() {
            if canonical_tool_is_responses_namespace(tool) {
                result.namespace_tool_indices.insert(tool_index);
                match parse_namespace_tool(tool_index, tool) {
                    Some(children) => {
                        for child in &children {
                            *name_counts.entry(child.name.clone()).or_default() += 1;
                        }
                        parsed.extend(children);
                    }
                    None => {
                        result.invalid_namespace_tool_indices.insert(tool_index);
                    }
                }
            } else {
                *name_counts.entry(tool.name.clone()).or_default() += 1;
                ordinary_chat_names.insert(tool.name.clone());
            }
        }

        let mut sources_by_identity = BTreeMap::<(String, String), BTreeSet<usize>>::new();
        for child in &parsed {
            let identity = (child.namespace.clone(), child.name.clone());
            sources_by_identity
                .entry(identity)
                .or_default()
                .insert(child.source_tool_index);
        }
        for sources in sources_by_identity
            .values()
            .filter(|sources| sources.len() > 1)
        {
            result
                .invalid_namespace_tool_indices
                .extend(sources.iter().copied());
        }
        parsed.retain(|child| {
            !result
                .invalid_namespace_tool_indices
                .contains(&child.source_tool_index)
        });

        let preferred_names = parsed
            .iter()
            .map(|child| {
                let name_is_globally_unique = name_counts.get(&child.name).copied() == Some(1);
                let preferred = if name_is_globally_unique
                    && is_valid_chat_tool_name(&child.name)
                    && !ordinary_chat_names.contains(&child.name)
                {
                    Some(child.name.clone())
                } else {
                    let readable = format!("{}__{}", child.namespace, child.name);
                    (is_valid_chat_tool_name(&readable) && !ordinary_chat_names.contains(&readable))
                        .then_some(readable)
                };
                ((child.namespace.clone(), child.name.clone()), preferred)
            })
            .collect::<BTreeMap<_, _>>();
        let mut preferred_counts = BTreeMap::<String, usize>::new();
        for preferred in preferred_names.values().flatten() {
            *preferred_counts.entry(preferred.clone()).or_default() += 1;
        }

        let mut aliases_by_identity = BTreeMap::<(String, String), String>::new();
        let mut used_chat_names = ordinary_chat_names;
        for (identity, preferred) in &preferred_names {
            if let Some(preferred) = preferred
                .as_ref()
                .filter(|name| preferred_counts.get(*name).copied() == Some(1))
            {
                aliases_by_identity.insert(identity.clone(), preferred.clone());
                used_chat_names.insert(preferred.clone());
            }
        }
        for identity in preferred_names.keys() {
            if aliases_by_identity.contains_key(identity) {
                continue;
            }
            let chat_name = allocate_hashed_alias(&identity.0, &identity.1, &used_chat_names);
            used_chat_names.insert(chat_name.clone());
            aliases_by_identity.insert(identity.clone(), chat_name);
        }

        for mut child in parsed {
            let identity = (child.namespace.clone(), child.name.clone());
            let chat_name = aliases_by_identity
                .get(&identity)
                .expect("every valid namespace child receives an alias")
                .clone();
            child.chat_name = chat_name.clone();
            result
                .by_identity
                .insert(identity.clone(), chat_name.clone());
            result.by_chat_name.insert(chat_name, identity);
            result.tools.push(child);
        }

        result
    }

    pub(crate) fn from_report_context(report_context: &Value) -> Self {
        let Some(tools) = report_context
            .get("original_request_body")
            .and_then(|request| request.get("tools"))
        else {
            return Self::default();
        };
        let client_function_tool_names = client_function_tool_names(tools);
        let mut result = openai_responses_tools_to_canonical(Some(tools))
            .map(|canonical| Self::from_canonical_tools(&canonical))
            .unwrap_or_default();
        result.client_function_tool_names = client_function_tool_names;
        result
    }

    /// Whether a tool call named `name` should surface to a Responses client as
    /// a hosted `web_search_call`. A client that declared its own function or
    /// custom tool called `web_search` must get a `function_call` back, or it
    /// cannot answer the call and will echo an unconvertible hosted item.
    /// When the client declares both a hosted `web_search` tool and a function
    /// of the same name, the function wins: only the client can answer it.
    pub(crate) fn emits_hosted_web_search_call(&self, name: &str) -> bool {
        matches!(name, "web_search" | "web_search_preview")
            && self.responses_name(name).is_none()
            && !self.client_function_tool_names.contains(name)
    }

    pub(crate) fn chat_name(&self, namespace: &str, child_name: &str) -> Option<&str> {
        self.by_identity
            .get(&(namespace.to_string(), child_name.to_string()))
            .map(String::as_str)
    }

    pub(crate) fn responses_name(&self, chat_name: &str) -> Option<(&str, &str)> {
        self.by_chat_name
            .get(chat_name)
            .map(|(namespace, child_name)| (namespace.as_str(), child_name.as_str()))
    }

    pub(crate) fn tools_for_source(
        &self,
        source_tool_index: usize,
    ) -> impl Iterator<Item = &NamespaceChatTool> {
        self.tools
            .iter()
            .filter(move |tool| tool.source_tool_index == source_tool_index)
    }

    pub(crate) fn is_namespace_tool(&self, source_tool_index: usize) -> bool {
        self.namespace_tool_indices.contains(&source_tool_index)
    }

    pub(crate) fn is_representable_namespace_tool(&self, source_tool_index: usize) -> bool {
        self.is_namespace_tool(source_tool_index)
            && !self
                .invalid_namespace_tool_indices
                .contains(&source_tool_index)
            && self
                .tools
                .iter()
                .any(|tool| tool.source_tool_index == source_tool_index)
    }

    pub(crate) fn has_invalid_namespace_tools(&self) -> bool {
        !self.invalid_namespace_tool_indices.is_empty()
    }

    pub(crate) fn namespace_children_named<'a>(
        &'a self,
        child_name: &'a str,
    ) -> impl Iterator<Item = &'a NamespaceChatTool> + 'a {
        self.tools
            .iter()
            .filter(move |tool| tool.name == child_name)
    }
}

fn client_function_tool_names(tools: &Value) -> BTreeSet<String> {
    tools
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(|tool_type| {
                    tool_type.eq_ignore_ascii_case("function")
                        || tool_type.eq_ignore_ascii_case("custom")
                })
        })
        .filter_map(|tool| {
            non_empty_string(tool.get("name")).or_else(|| {
                ["function", "custom"]
                    .iter()
                    .find_map(|key| non_empty_string(tool.get(*key)?.get("name")))
            })
        })
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn canonical_tool_is_responses_namespace(tool: &CanonicalToolDefinition) -> bool {
    raw_responses_tool(tool).is_some_and(|raw| {
        raw.get("type")
            .and_then(Value::as_str)
            .is_some_and(|tool_type| tool_type.eq_ignore_ascii_case("namespace"))
    })
}

fn raw_responses_tool(tool: &CanonicalToolDefinition) -> Option<&Map<String, Value>> {
    tool.extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            tool.extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(Value::as_object)
}

fn parse_namespace_tool(
    source_tool_index: usize,
    tool: &CanonicalToolDefinition,
) -> Option<Vec<NamespaceChatTool>> {
    let raw = raw_responses_tool(tool)?;
    if !object_has_only_keys(raw, &["type", "name", "description", "tools"]) {
        return None;
    }
    let namespace = non_empty_string(raw.get("name"))?.to_string();
    if !matches!(raw.get("description"), Some(Value::String(_))) {
        return None;
    }
    let children = raw.get("tools")?.as_array()?;
    if children.is_empty() {
        return None;
    }

    let mut names = BTreeSet::new();
    let mut parsed = Vec::with_capacity(children.len());
    for (source_child_index, child) in children.iter().enumerate() {
        let child = child.as_object()?;
        if child.get("type").and_then(Value::as_str) != Some("function") {
            return None;
        }
        if !object_has_only_keys(
            child,
            &["type", "name", "description", "parameters", "strict"],
        ) {
            return None;
        }
        let function = child;
        let name = non_empty_string(function.get("name"))?.to_string();
        if !names.insert(name.clone()) {
            return None;
        }
        // Responses permits an omitted or null parameter schema, while Chat
        // Completions only permits an omitted schema or an object. Treat null
        // as the omitted form instead of forwarding an invalid
        // `parameters: null` Chat tool definition.
        let parameters = match function.get("parameters") {
            Some(parameters @ Value::Object(_)) => Some(parameters.clone()),
            Some(Value::Null) | None => None,
            Some(_) => return None,
        };
        let strict = match function.get("strict") {
            Some(strict @ (Value::Bool(_) | Value::Null)) => Some(strict.clone()),
            Some(_) => return None,
            None => None,
        };
        let description = match function.get("description") {
            Some(Value::String(description)) => Some(description.clone()),
            Some(Value::Null) | None => None,
            Some(_) => return None,
        };
        parsed.push(NamespaceChatTool {
            source_tool_index,
            source_child_index,
            namespace: namespace.clone(),
            name,
            chat_name: String::new(),
            description,
            parameters,
            strict,
        });
    }
    Some(parsed)
}

fn object_has_only_keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_valid_chat_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= OPENAI_CHAT_TOOL_NAME_MAX_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn allocate_hashed_alias(namespace: &str, child_name: &str, used: &BTreeSet<String>) -> String {
    for nonce in 0_u64.. {
        let candidate = hashed_alias(namespace, child_name, nonce);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 alias nonce space cannot be exhausted")
}

fn sanitize_chat_name_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect()
}

fn hashed_alias(namespace: &str, child_name: &str, nonce: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aether-openai-namespace-tool\0");
    hasher.update(namespace.as_bytes());
    hasher.update(b"\0");
    hasher.update(child_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(nonce.to_le_bytes());
    let digest = hasher.finalize();
    let digest_hex = digest
        .iter()
        .take(HASH_HEX_LEN / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let semantic_budget =
        OPENAI_CHAT_TOOL_NAME_MAX_LEN - HASHED_ALIAS_PREFIX.len() - 1 - digest_hex.len();
    let mut semantic = sanitize_chat_name_component(child_name);
    semantic.truncate(semantic_budget);
    if semantic.is_empty() {
        semantic.push_str("tool");
        semantic.truncate(semantic_budget);
    }
    format!("{HASHED_ALIAS_PREFIX}{semantic}_{digest_hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::canonical::openai_responses_tools_to_canonical;

    fn aliases(tools: Value) -> NamespaceToolAliases {
        let canonical = openai_responses_tools_to_canonical(Some(&tools))
            .expect("Responses tools should parse");
        NamespaceToolAliases::from_canonical_tools(&canonical)
    }

    #[test]
    fn namespace_aliases_keep_unique_child_names_and_reverse_them() {
        let aliases = aliases(json!([{
            "type": "namespace",
            "name": "mcp__reports",
            "description": "Reporting tools",
            "tools": [{
                "type": "function",
                "name": "vulnerability_report",
                "parameters": {"type": "object", "properties": {}}
            }]
        }]));

        assert_eq!(
            aliases.chat_name("mcp__reports", "vulnerability_report"),
            Some("vulnerability_report")
        );
        assert_eq!(
            aliases.responses_name("vulnerability_report"),
            Some(("mcp__reports", "vulnerability_report"))
        );
    }

    fn hosted_web_search(tools: Value, name: &str) -> bool {
        NamespaceToolAliases::from_report_context(&json!({
            "original_request_body": {"tools": tools}
        }))
        .emits_hosted_web_search_call(name)
    }

    #[test]
    fn hosted_web_search_call_is_reserved_for_undeclared_search_names() {
        let schema = json!({"type": "object", "properties": {"query": {"type": "string"}}});
        let cases = [
            ("no tools", json!([]), "web_search", true),
            (
                "hosted tool",
                json!([{"type": "web_search"}]),
                "web_search",
                true,
            ),
            (
                "hosted preview tool",
                json!([{"type": "web_search_preview"}]),
                "web_search_preview",
                true,
            ),
            (
                "function tool",
                json!([{"type": "function", "name": "web_search", "parameters": schema}]),
                "web_search",
                false,
            ),
            (
                "function tool named preview",
                json!([{"type": "function", "name": "web_search_preview", "parameters": schema}]),
                "web_search_preview",
                false,
            ),
            (
                "custom tool",
                json!([{"type": "custom", "name": "web_search"}]),
                "web_search",
                false,
            ),
            (
                "tool without type",
                json!([{"name": "web_search", "parameters": schema}]),
                "web_search",
                false,
            ),
            (
                "chat-shaped function tool",
                json!([{"type": "function", "function": {"name": "web_search", "parameters": schema}}]),
                "web_search",
                false,
            ),
            (
                "chat-shaped custom tool",
                json!([{"type": "custom", "custom": {"name": "web_search"}}]),
                "web_search",
                false,
            ),
            (
                "hosted and function tool together",
                json!([
                    {"type": "web_search"},
                    {"type": "function", "name": "web_search", "parameters": schema}
                ]),
                "web_search",
                false,
            ),
            (
                "unrelated function tool",
                json!([{"type": "function", "name": "lookup", "parameters": schema}]),
                "web_search",
                true,
            ),
            (
                "non-search name",
                json!([{"type": "web_search"}]),
                "lookup",
                false,
            ),
        ];

        for (label, tools, name, expected) in cases {
            assert_eq!(hosted_web_search(tools, name), expected, "{label}");
        }
    }

    #[test]
    fn namespaced_web_search_child_is_not_a_hosted_web_search_call() {
        assert!(!hosted_web_search(
            json!([{
                "type": "namespace",
                "name": "mcp__search",
                "description": "Search tools",
                "tools": [{
                    "type": "function",
                    "name": "web_search",
                    "parameters": {"type": "object", "properties": {}}
                }]
            }]),
            "web_search"
        ));
    }

    #[test]
    fn namespace_aliases_are_unique_bounded_and_prefix_safe() {
        let long_namespace = format!("namespace__{}", "n".repeat(120));
        let long_child = format!("aether_ns__{}", "c".repeat(120));
        let tools = json!([
            {
                "type": "function",
                "name": "shared",
                "parameters": {"type": "object"}
            },
            {
                "type": "namespace",
                "name": "first__namespace",
                "description": "First namespace",
                "tools": [{
                    "type": "function",
                    "name": "shared",
                    "parameters": {"type": "object"}
                }]
            },
            {
                "type": "namespace",
                "name": "second__namespace",
                "description": "Second namespace",
                "tools": [{
                    "type": "function",
                    "name": "shared",
                    "parameters": {"type": "object"}
                }]
            },
            {
                "type": "namespace",
                "name": long_namespace,
                "description": "Long namespace",
                "tools": [{
                    "type": "function",
                    "name": long_child,
                    "parameters": {"type": "object"}
                }]
            }
        ]);
        let aliases = aliases(tools);
        let names = aliases
            .tools
            .iter()
            .map(|tool| tool.chat_name.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(names.len(), 3);
        assert!(!names.contains("shared"));
        assert!(names.iter().all(|name| is_valid_chat_tool_name(name)));
        assert!(names.iter().all(|name| name.len() <= 64));
        for tool in &aliases.tools {
            assert_eq!(
                aliases.responses_name(&tool.chat_name),
                Some((tool.namespace.as_str(), tool.name.as_str()))
            );
        }
    }

    #[test]
    fn namespace_aliases_are_stable_across_tool_order_and_readable_collisions() {
        let first = json!([
            {
                "type": "function",
                "name": "b__c",
                "parameters": {"type": "object"}
            },
            {
                "type": "function",
                "name": "c",
                "parameters": {"type": "object"}
            },
            {
                "type": "namespace",
                "name": "a",
                "description": "A",
                "tools": [{
                    "type": "function",
                    "name": "b__c",
                    "parameters": {"type": "object"}
                }]
            },
            {
                "type": "namespace",
                "name": "a__b",
                "description": "AB",
                "tools": [{
                    "type": "function",
                    "name": "c",
                    "parameters": {"type": "object"}
                }]
            }
        ]);
        let second = json!([
            first[3].clone(),
            first[2].clone(),
            first[1].clone(),
            first[0].clone()
        ]);
        let first = aliases(first);
        let second = aliases(second);

        for identity in [("a", "b__c"), ("a__b", "c")] {
            let first_alias = first
                .chat_name(identity.0, identity.1)
                .expect("first alias");
            let second_alias = second
                .chat_name(identity.0, identity.1)
                .expect("second alias");
            assert_eq!(first_alias, second_alias);
            assert!(first_alias.starts_with(HASHED_ALIAS_PREFIX));
        }
    }

    #[test]
    fn namespace_children_preserve_nullable_fields_without_inheriting_parent_description() {
        let aliases = aliases(json!([{
            "type": "namespace",
            "name": "reports",
            "description": "Parent description",
            "tools": [{
                "type": "function",
                "name": "write_report",
                "description": null,
                "parameters": null,
                "strict": null
            }]
        }]));
        let chat_tool = aliases
            .tools_for_source(0)
            .next()
            .expect("namespace child")
            .to_openai_chat_tool();

        assert!(chat_tool["function"].get("description").is_none());
        assert!(chat_tool["function"].get("parameters").is_none());
        assert_eq!(chat_tool["function"]["strict"], Value::Null);
    }

    #[test]
    fn malformed_namespace_is_not_representable() {
        for raw in [
            json!({"type": "namespace", "name": "broken"}),
            json!({
                "type": "namespace",
                "name": "broken",
                "tools": [{"type": "function", "name": "missing_parent_description"}]
            }),
            json!({
                "type": "namespace",
                "name": "broken",
                "description": "Broken",
                "tools": {}
            }),
            json!({
                "type": "namespace",
                "name": "broken",
                "description": "Broken",
                "tools": [{"type": "function"}]
            }),
            json!({
                "type": "namespace",
                "name": "broken",
                "description": "Broken",
                "tools": [{"type": "custom", "name": "raw"}]
            }),
            json!({
                "type": "namespace",
                "name": "broken",
                "description": "Broken",
                "tools": [{"name": "missing_type"}]
            }),
            json!({
                "type": "namespace",
                "name": "broken",
                "description": "Broken",
                "tools": [{
                    "type": "function",
                    "function": {"name": "nested"}
                }]
            }),
        ] {
            let aliases = aliases(json!([raw]));
            assert!(aliases.has_invalid_namespace_tools());
            assert!(!aliases.is_representable_namespace_tool(0));
        }
    }
}
