use serde_json::Value;

const REQUEST_FIELDS: &[&str] = &[
    "id",
    "model",
    "reasoning",
    "input",
    "commands",
    "settings",
    "max_output_tokens",
];
const REASONING_FIELDS: &[&str] = &["effort", "summary", "context"];
const COMMAND_FIELDS: &[&str] = &[
    "search_query",
    "image_query",
    "open",
    "click",
    "find",
    "screenshot",
    "finance",
    "weather",
    "sports",
    "time",
    "response_length",
];
const COMMAND_ITEM_FIELDS: &[(&str, &[&str])] = &[
    ("search_query", &["q", "recency", "domains"]),
    ("image_query", &["q", "recency", "domains"]),
    ("open", &["ref_id", "lineno"]),
    ("click", &["ref_id", "id"]),
    ("find", &["ref_id", "pattern"]),
    ("screenshot", &["ref_id", "pageno"]),
    ("finance", &["ticker", "type", "market"]),
    ("weather", &["location", "start", "duration"]),
    (
        "sports",
        &[
            "tool",
            "fn",
            "league",
            "team",
            "opponent",
            "date_from",
            "date_to",
            "num_games",
            "locale",
        ],
    ),
    ("time", &["utc_offset"]),
];
const SETTINGS_FIELDS: &[&str] = &[
    "user_location",
    "search_context_size",
    "filters",
    "image_settings",
    "allowed_callers",
    "external_web_access",
];
const USER_LOCATION_FIELDS: &[&str] = &["type", "country", "region", "city", "timezone"];
const FILTER_FIELDS: &[&str] = &["allowed_domains", "blocked_domains"];
const IMAGE_SETTINGS_FIELDS: &[&str] = &["max_results", "caption"];

fn retain_object_fields(value: &mut Value, fields: &[&str]) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|field, _| fields.contains(&field.as_str()));
    }
}

fn retain_array_object_fields(
    object: &mut serde_json::Map<String, Value>,
    key: &str,
    fields: &[&str],
) {
    if let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) {
        for item in items {
            retain_object_fields(item, fields);
        }
    }
}

pub fn apply_openai_search_request_projection(body: &mut Value, provider_api_format: &str) {
    if !crate::api_format_alias_matches(provider_api_format, "openai:search") {
        return;
    }
    let Some(body_object) = body.as_object_mut() else {
        return;
    };
    body_object.retain(|field, _| REQUEST_FIELDS.contains(&field.as_str()));

    if let Some(reasoning) = body_object.get_mut("reasoning") {
        retain_object_fields(reasoning, REASONING_FIELDS);
    }
    if let Some(commands) = body_object
        .get_mut("commands")
        .and_then(Value::as_object_mut)
    {
        commands.retain(|field, _| COMMAND_FIELDS.contains(&field.as_str()));
        for (key, fields) in COMMAND_ITEM_FIELDS {
            retain_array_object_fields(commands, key, fields);
        }
    }
    if let Some(settings) = body_object
        .get_mut("settings")
        .and_then(Value::as_object_mut)
    {
        settings.retain(|field, _| SETTINGS_FIELDS.contains(&field.as_str()));
        if let Some(user_location) = settings.get_mut("user_location") {
            retain_object_fields(user_location, USER_LOCATION_FIELDS);
        }
        if let Some(filters) = settings.get_mut("filters") {
            retain_object_fields(filters, FILTER_FIELDS);
        }
        if let Some(image_settings) = settings.get_mut("image_settings") {
            retain_object_fields(image_settings, IMAGE_SETTINGS_FIELDS);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::apply_openai_search_request_projection;

    #[test]
    fn projects_the_typed_search_request_contract() {
        let mut body = json!({
            "id": "session-1",
            "model": "gpt-5.6-sol",
            "reasoning": {
                "effort": "max",
                "summary": "auto",
                "context": "current_turn",
                "mode": "pro"
            },
            "input": "find documentation",
            "commands": {
                "search_query": [{"q": "Aether", "recency": 7, "unknown": true}],
                "image_query": [{"q": "Aether UI", "domains": ["example.com"], "unknown": true}],
                "open": [{"ref_id": "turn0search0", "lineno": 12, "unknown": true}],
                "click": [{"ref_id": "turn0fetch0", "id": 3, "unknown": true}],
                "find": [{"ref_id": "turn0fetch0", "pattern": "Aether", "unknown": true}],
                "screenshot": [{"ref_id": "turn0fetch0", "pageno": 2, "unknown": true}],
                "finance": [{"ticker": "OPENAI", "type": "equity", "market": "USA", "unknown": true}],
                "weather": [{"location": "US, CA, San Francisco", "duration": 3, "unknown": true}],
                "sports": [{"tool": "sports", "fn": "schedule", "league": "nba", "team": "GSW", "unknown": true}],
                "time": [{"utc_offset": "+08:00", "unknown": true}],
                "response_length": "short",
                "unknown": true
            },
            "settings": {
                "user_location": {"type": "approximate", "country": "US", "unknown": true},
                "search_context_size": "high",
                "filters": {"allowed_domains": ["openai.com"], "unknown": true},
                "image_settings": {"max_results": 3, "unknown": true},
                "allowed_callers": ["direct"],
                "external_web_access": "live",
                "unknown": true
            },
            "max_output_tokens": 1024,
            "store": false,
            "stream": true,
            "service_tier": "priority",
            "unknown": true
        });

        apply_openai_search_request_projection(&mut body, "/v1/alpha/search");

        assert_eq!(
            body["reasoning"],
            json!({
                "effort": "max",
                "summary": "auto",
                "context": "current_turn"
            })
        );
        assert_eq!(
            body["commands"],
            json!({
                "search_query": [{"q": "Aether", "recency": 7}],
                "image_query": [{"q": "Aether UI", "domains": ["example.com"]}],
                "open": [{"ref_id": "turn0search0", "lineno": 12}],
                "click": [{"ref_id": "turn0fetch0", "id": 3}],
                "find": [{"ref_id": "turn0fetch0", "pattern": "Aether"}],
                "screenshot": [{"ref_id": "turn0fetch0", "pageno": 2}],
                "finance": [{"ticker": "OPENAI", "type": "equity", "market": "USA"}],
                "weather": [{"location": "US, CA, San Francisco", "duration": 3}],
                "sports": [{"tool": "sports", "fn": "schedule", "league": "nba", "team": "GSW"}],
                "time": [{"utc_offset": "+08:00"}],
                "response_length": "short"
            })
        );
        assert_eq!(
            body["settings"],
            json!({
                "user_location": {"type": "approximate", "country": "US"},
                "search_context_size": "high",
                "filters": {"allowed_domains": ["openai.com"]},
                "image_settings": {"max_results": 3},
                "allowed_callers": ["direct"],
                "external_web_access": "live"
            })
        );
        assert!(body.get("store").is_none());
        assert!(body.get("stream").is_none());
        assert!(body.get("service_tier").is_none());
        assert!(body.get("unknown").is_none());
    }

    #[test]
    fn leaves_other_formats_unchanged() {
        let mut body = json!({"model": "gpt-5.6-sol", "store": true});
        let expected = body.clone();

        apply_openai_search_request_projection(&mut body, "openai:responses");

        assert_eq!(body, expected);
    }
}
