//! Rhai-based custom quota script evaluation.
//!
//! A provider's quota query is configured as a single Rhai script:
//!
//! ```rhai
//! #{
//!     request: #{
//!         url: "https://api.example.com/user/balance",
//!         method: "GET",
//!         headers: #{ "Authorization": "Bearer {{apiKey}}" },
//!         body: #{ "query": "quota { remaining }" }
//!     },
//!     extractor: |response| #{
//!         remaining: response.balance,
//!         unit: "USD",
//!         periods: #[
//!             #{ key: "5h", used: 2.5, limit: 10 },
//!             #{ key: "week", used: 30, limit: 50, reset: "2026-11-02T00:00:00Z" },
//!             #{ key: "month", used: 57.5, limit: 100 }
//!         ]
//!     }
//! }
//! ```
//!
//! The engine is fully sandboxed: Rhai ships no I/O, every evaluation gets a
//! fresh engine with operation / call-level / string-size limits plus a wall
//! clock deadline. `{{apiKey}}` placeholders are substituted by the caller
//! (gateway) before the HTTP request is issued.

use rhai::{Dynamic, Engine, FnPtr, Map, AST};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const MAX_SCRIPT_BYTES: usize = 16 * 1024;
const MAX_URL_LEN: usize = 2048;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_VALUE_LEN: usize = 2048;
const MAX_TEXT_FIELD_LEN: usize = 256;
const MAX_BODY_BYTES: usize = 8 * 1024;
const MAX_OPERATIONS: u64 = 100_000;
const MAX_CALL_LEVELS: usize = 32;
const MAX_STRING_SIZE: usize = 64 * 1024;
const EXTRACTOR_TIMEOUT: Duration = Duration::from_millis(2_000);

/// Request definition extracted from a quota script. The URL must be a full
/// http/https address (optionally embedding `{{apiKey}}`); header values and
/// the optional JSON body keep their `{{apiKey}}` placeholders for the caller
/// to resolve.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaScriptRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    /// Optional JSON body (Rhai map / array / string containing JSON). Sent
    /// with `application/json`.
    pub body: Option<Value>,
}

impl QuotaScriptRequest {
    /// Whether any request part references the `{{apiKey}}` placeholder, so
    /// the caller knows credential resolution is required.
    pub fn references_api_key(&self) -> bool {
        let hit = |value: &str| value.contains("{{apiKey}}") || value.contains("{{api_key}}");
        hit(&self.url)
            || self.headers.values().any(|value| hit(value))
            || self
                .body
                .as_ref()
                .is_some_and(value_tree_references_api_key)
    }
}

fn value_tree_references_api_key(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains("{{apiKey}}") || text.contains("{{api_key}}"),
        Value::Array(items) => items.iter().any(value_tree_references_api_key),
        Value::Object(map) => map.values().any(value_tree_references_api_key),
        _ => false,
    }
}

fn sandboxed_engine() -> Engine {
    let deadline = Instant::now() + EXTRACTOR_TIMEOUT;
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_string_size(MAX_STRING_SIZE);
    engine.on_progress(move |_| {
        (Instant::now() >= deadline).then(|| Dynamic::from("额度查询脚本执行超时"))
    });
    engine
}

fn evaluate_config(
    engine: &Engine,
    script: &str,
) -> Result<(QuotaScriptRequest, FnPtr, AST), String> {
    let ast = engine
        .compile(script)
        .map_err(|err| format!("脚本语法错误: {err}"))?;
    let value: Dynamic = engine
        .eval_ast::<Dynamic>(&ast)
        .map_err(|err| format!("脚本执行失败: {err}"))?;
    let map = value
        .try_cast::<Map>()
        .ok_or_else(|| "脚本必须返回 #{ request: ..., extractor: ... } 对象".to_string())?;

    let request = map
        .get("request")
        .cloned()
        .and_then(|value| value.try_cast::<Map>())
        .ok_or_else(|| "缺少 request 配置".to_string())?;
    let url = string_field(&request, "url")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "request.url 不能为空".to_string())?;
    if url.len() > MAX_URL_LEN || url.contains('\n') || url.contains('\r') {
        return Err("request.url 无效".to_string());
    }
    // The URL must be a full http/https address; embedding an api-key
    // placeholder is the only accepted non-absolute form (resolved by the
    // caller at execution time).
    let embeds_placeholder = url.contains("{{apiKey}}") || url.contains("{{api_key}}");
    if !embeds_placeholder {
        let parsed =
            url::Url::parse(&url).map_err(|_| "request.url 必须是完整的 http/https 地址")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("request.url 仅支持 http / https".to_string());
        }
    }
    let method = match string_field(&request, "method") {
        Some(raw) => {
            let value = raw.trim().to_ascii_uppercase();
            if matches!(value.as_str(), "GET" | "POST" | "PUT" | "PATCH") {
                value
            } else {
                return Err("request.method 仅支持 GET / POST / PUT / PATCH".to_string());
            }
        }
        None => "GET".to_string(),
    };
    let mut headers = BTreeMap::new();
    if let Some(header_map) = request
        .get("headers")
        .cloned()
        .and_then(|v| v.try_cast::<Map>())
    {
        if header_map.len() > MAX_HEADERS {
            return Err("request.headers 条目过多".to_string());
        }
        for (name, value) in header_map {
            let Some(value) = value.try_cast::<String>() else {
                continue;
            };
            let name = name.trim();
            if name.is_empty()
                || name.len() > MAX_TEXT_FIELD_LEN
                || name.contains('\n')
                || value.len() > MAX_HEADER_VALUE_LEN
                || value.contains('\n')
            {
                return Err("request.headers 键或值无效".to_string());
            }
            headers.insert(name.to_string(), value);
        }
    }
    let body = parse_request_body(request.get("body"))?;

    let extractor = map
        .get("extractor")
        .cloned()
        .and_then(|value| value.try_cast::<FnPtr>())
        .ok_or_else(|| "extractor 必须是函数，形如 |response| #{ ... }".to_string())?;

    Ok((
        QuotaScriptRequest {
            url,
            method,
            headers,
            body,
        },
        extractor,
        ast,
    ))
}

/// Parse the optional `request.body`: a Rhai map / array converts to JSON
/// directly, a Rhai string must contain valid JSON (so the gateway can always
/// send `application/json`). Placeholders stay untouched for the caller.
fn parse_request_body(raw: Option<&Dynamic>) -> Result<Option<Value>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_unit() {
        return Ok(None);
    }
    let converted = rhai::serde::from_dynamic::<Value>(raw)
        .map_err(|err| format!("request.body 无法转换为 JSON: {err}"))?;
    let body = match converted {
        Value::String(text) => serde_json::from_str::<Value>(text.trim())
            .map_err(|_| "request.body 字符串必须是合法的 JSON".to_string())?,
        other => other,
    };
    if serde_json::to_string(&body).map_or(true, |text| text.len() > MAX_BODY_BYTES) {
        return Err(format!(
            "request.body 过大，上限 {}KB",
            MAX_BODY_BYTES / 1024
        ));
    }
    Ok(Some(body))
}

fn string_field(map: &Map, key: &str) -> Option<String> {
    map.get(key)
        .cloned()
        .and_then(|value| value.try_cast::<String>())
        .map(|value| value.trim().to_string())
}

/// Validate a quota script and return its request definition.
pub fn validate_quota_script(script: &str) -> Result<QuotaScriptRequest, String> {
    let script = script.trim();
    if script.is_empty() {
        return Err("脚本不能为空".to_string());
    }
    if script.len() > MAX_SCRIPT_BYTES {
        return Err("脚本长度不能超过 16KB".to_string());
    }
    let engine = sandboxed_engine();
    let (request, _, _) = evaluate_config(&engine, script)?;
    Ok(request)
}

/// Run a quota script's extractor against a response document and project the
/// result onto the known optional fields (`is_valid`, `invalid_message`,
/// `remaining`, `unit`, `plan_name`, `total`, `used`, `extra`).
pub fn run_quota_extractor(script: &str, response: &Value) -> Result<Value, String> {
    let script = script.trim();
    if script.is_empty() {
        return Err("脚本不能为空".to_string());
    }
    let engine = sandboxed_engine();
    let (_, extractor, ast) = evaluate_config(&engine, script)?;
    let response_dynamic =
        rhai::serde::to_dynamic(response).map_err(|err| format!("响应无法传入脚本: {err}"))?;
    let result: Dynamic = extractor
        .call(&engine, &ast, (response_dynamic,))
        .map_err(|err| format!("extractor 执行失败: {err}"))?;
    let map = result
        .try_cast::<Map>()
        .ok_or_else(|| "extractor 必须返回 #{ ... } 对象".to_string())?;
    Ok(project_quota_fields(&map))
}

fn project_quota_fields(map: &Map) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(value) = map
        .get("isValid")
        .cloned()
        .and_then(|v| v.try_cast::<bool>())
    {
        out.insert("is_valid".to_string(), json!(value));
    }
    for (field, key) in [
        ("invalidMessage", "invalid_message"),
        ("planName", "plan_name"),
        ("unit", "unit"),
        ("extra", "extra"),
    ] {
        if let Some(text) = string_field(map, field) {
            if !text.is_empty() && text.len() <= MAX_TEXT_FIELD_LEN {
                out.insert(key.to_string(), json!(text));
            }
        }
    }
    for field in ["remaining", "total", "used"] {
        if let Some(number) = map
            .get(field)
            .and_then(dynamic_as_f64)
            .filter(|value| value.is_finite())
        {
            out.insert(field.to_string(), json!(number));
        }
    }
    if let Some(periods) = project_quota_periods(map) {
        out.insert("periods".to_string(), periods);
    }
    Value::Object(out)
}

const MAX_PERIODS: usize = 8;
const MAX_PERIOD_KEY_LEN: usize = 32;

/// Project the optional `periods` array (5h / week / month usage windows)
/// onto `{ key, used, limit, reset_at }` entries. `reset` accepts an epoch
/// number, an RFC3339 string, or free display text (passed through as-is).
fn project_quota_periods(map: &Map) -> Option<Value> {
    let items = map.get("periods")?.clone().try_cast::<rhai::Array>()?;
    let mut periods = Vec::new();
    for item in items.iter().take(MAX_PERIODS) {
        let Some(period) = item.clone().try_cast::<Map>() else {
            continue;
        };
        let Some(key) = string_field(&period, "key")
            .filter(|key| !key.is_empty() && key.len() <= MAX_PERIOD_KEY_LEN)
        else {
            continue;
        };
        periods.push(json!({
            "key": key,
            "used": period.get("used").and_then(dynamic_as_f64),
            "limit": period.get("limit").and_then(dynamic_as_f64),
            "reset_at": period.get("reset").and_then(normalize_reset_at),
        }));
    }
    (!periods.is_empty()).then(|| Value::Array(periods))
}

fn normalize_reset_at(value: &Dynamic) -> Option<String> {
    if let Some(text) = value.clone().try_cast::<String>() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return format_reset_text(text);
    }
    let secs = normalize_epoch_candidate(value.clone().try_cast::<i64>()?)?;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

const MAX_EPOCH_SECS: i64 = 253_402_300_799;

/// Accept epoch seconds or epoch milliseconds (e.g. `1792825200000`).
fn normalize_epoch_candidate(value: i64) -> Option<i64> {
    if (0..=MAX_EPOCH_SECS).contains(&value) {
        return Some(value);
    }
    let millis = value / 1000;
    (0..=MAX_EPOCH_SECS).contains(&millis).then_some(millis)
}

fn format_reset_text(text: &str) -> Option<String> {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    if let Ok(secs) = text.parse::<i64>() {
        if let Some(secs) = normalize_epoch_candidate(secs) {
            return chrono::DateTime::from_timestamp(secs, 0)
                .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }
        return None;
    }
    // Free display text (e.g. "每月 1 日") passes through unchanged.
    (text.len() <= 256).then(|| text.to_string())
}

fn dynamic_as_f64(value: &Dynamic) -> Option<f64> {
    if value.is_int() {
        return value.as_int().ok().map(|v| v as f64);
    }
    if value.is_float() {
        return value.as_float().ok();
    }
    value
        .clone()
        .try_cast::<String>()
        .and_then(|text| text.trim().parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use super::{run_quota_extractor, validate_quota_script};
    use serde_json::json;

    const VALID_SCRIPT: &str = r#"
        #{
            request: #{
                url: "https://api.example.com/user/balance",
                method: "GET",
                headers: #{ "Authorization": "Bearer {{apiKey}}" }
            },
            extractor: |response| #{
                isValid: response.is_active,
                remaining: response.balance,
                total: response.quota_total,
                used: response.quota_used,
                unit: "USD",
                planName: "Pro",
                invalidMessage: "",
                extra: response.note,
                periods: [
                    #{ key: "5h", used: response.window_5h.used, limit: response.window_5h.limit, reset: response.window_5h.resets_at },
                    #{ key: "week", used: 30, limit: 50, reset: "2026-11-02T00:00:00Z" },
                    #{ key: "month", used: "57.5", limit: 100, reset: "每月 1 日" },
                    #{ key: "" },
                    "not-a-map"
                ]
            }
        }
    "#;

    #[test]
    fn validates_script_and_extracts_request() {
        let request = validate_quota_script(VALID_SCRIPT).expect("valid script");
        assert_eq!(request.url, "https://api.example.com/user/balance");
        assert_eq!(request.method, "GET");
        assert!(
            request.body.is_none(),
            "scripts without body should have no body"
        );
        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("Bearer {{apiKey}}")
        );
    }

    #[test]
    fn request_body_accepts_map_string_and_keeps_placeholders() {
        // Rhai map body converts to a JSON object.
        let map_script = r#"
            #{
                request: #{
                    url: "https://x.test/graphql",
                    method: "POST",
                    body: #{ query: "quota { used }", key: "{{apiKey}}" }
                },
                extractor: |r| #{}
            }
        "#;
        let request = validate_quota_script(map_script).expect("valid script");
        assert_eq!(
            request.body,
            Some(json!({ "query": "quota { used }", "key": "{{apiKey}}" }))
        );
        assert!(request.references_api_key(), "body placeholder counts");

        // String body must be valid JSON.
        let text_script = r#"
            #{
                request: #{
                    url: "https://x.test",
                    method: "POST",
                    body: "{\"model\":\"k2\"}"
                },
                extractor: |r| #{}
            }
        "#;
        let request = validate_quota_script(text_script).expect("valid script");
        assert_eq!(request.body, Some(json!({ "model": "k2" })));

        // PUT/PATCH methods are accepted alongside GET/POST.
        let put_script = r#"
            #{
                request: #{ url: "https://x.test", method: "PUT" },
                extractor: |r| #{}
            }
        "#;
        assert_eq!(
            validate_quota_script(put_script).expect("valid").method,
            "PUT"
        );
    }

    #[test]
    fn request_body_rejects_invalid_shapes() {
        let invalid_json = r#"
            #{
                request: #{ url: "https://x.test", body: "not json" },
                extractor: |r| #{}
            }
        "#;
        let error = validate_quota_script(invalid_json).expect_err("invalid JSON body");
        assert!(error.contains("request.body"), "unexpected error: {error}");

        let bad_method = r#"
            #{
                request: #{ url: "https://x.test", method: "FETCH" },
                extractor: |r| #{}
            }
        "#;
        let error = validate_quota_script(bad_method).expect_err("unsupported method");
        assert!(
            error.contains("request.method"),
            "unexpected error: {error}"
        );

        // Placeholder-only detection: url and headers without placeholders,
        // body carrying one.
        let body_only = r#"
            #{
                request: #{
                    url: "https://x.test",
                    body: #{ token: "{{apiKey}}" }
                },
                extractor: |r| #{}
            }
        "#;
        let request = validate_quota_script(body_only).expect("valid script");
        assert!(request.references_api_key());
        let no_placeholder = r#"
            #{
                request: #{ url: "https://x.test" },
                extractor: |r| #{}
            }
        "#;
        assert!(!validate_quota_script(no_placeholder)
            .expect("valid")
            .references_api_key());
    }

    #[test]
    fn rejects_malformed_scripts() {
        assert!(validate_quota_script("").is_err());
        assert!(validate_quota_script("not rhai {{{").is_err());
        // Missing request / extractor.
        assert!(validate_quota_script("#{ request: #{ url: \"https://x.test\" } }").is_err());
        assert!(validate_quota_script("#{ extractor: |r| #{ remaining: 1 } }").is_err());
        // Non-closure extractor.
        assert!(
            validate_quota_script("#{ request: #{ url: \"https://x.test\" }, extractor: 42 }")
                .is_err()
        );
        // Relative and legacy {{baseUrl}} URLs are rejected: the URL must be
        // a full http/https address (an embedded api-key placeholder is fine).
        assert!(validate_quota_script(
            "#{ request: #{ url: \"/user/balance\" }, extractor: |r| #{} }"
        )
        .is_err());
        assert!(validate_quota_script(
            "#{ request: #{ url: \"{{baseUrl}}/user/balance\" }, extractor: |r| #{} }"
        )
        .is_err());
        assert!(validate_quota_script(
            "#{ request: #{ url: \"https://x.test/key/{{apiKey}}\" }, extractor: |r| #{} }"
        )
        .is_ok());
    }

    #[test]
    fn extractor_runs_and_projects_fields() {
        let response = json!({
            "is_active": true,
            "balance": 12.5,
            "quota_total": "100",
            "quota_used": 87.25,
            "note": "hello",
            "window_5h": { "used": 2.5, "limit": 10, "resets_at": 1767225600 }
        });
        let data = run_quota_extractor(VALID_SCRIPT, &response).expect("extractor runs");
        assert_eq!(data["is_valid"], json!(true));
        assert_eq!(data["remaining"], json!(12.5));
        // Numeric strings are coerced.
        assert_eq!(data["total"], json!(100.0));
        assert_eq!(data["used"], json!(87.25));
        assert_eq!(data["unit"], json!("USD"));
        assert_eq!(data["plan_name"], json!("Pro"));
        assert_eq!(data["extra"], json!("hello"));
        // Empty strings are dropped.
        assert!(data.get("invalid_message").is_none());
    }

    #[test]
    fn extractor_projects_periods() {
        let response = json!({
            "is_active": true,
            "balance": 12.5,
            "window_5h": { "used": 2.5, "limit": 10, "resets_at": 1767225600 }
        });
        let data = run_quota_extractor(VALID_SCRIPT, &response).expect("extractor runs");
        let periods = data["periods"].as_array().expect("periods array");
        // Entries without a valid key and non-map items are skipped.
        assert_eq!(periods.len(), 3);
        assert_eq!(periods[0]["key"], json!("5h"));
        assert_eq!(periods[0]["used"], json!(2.5));
        assert_eq!(periods[0]["limit"], json!(10.0));
        // Epoch numbers normalize to RFC3339.
        assert_eq!(periods[0]["reset_at"], json!("2026-01-01T00:00:00.000Z"));
        // Epoch milliseconds (e.g. MiniMAX window timestamps) also normalize.
        let ms_script = "#{ request: #{ url: \"https://x.test\" }, extractor: |r| #{ periods: [#{ key: \"5h\", reset: r.end_time }] } }";
        let data = run_quota_extractor(ms_script, &json!({ "end_time": 1792825200000_i64 }))
            .expect("extractor runs");
        assert_eq!(
            data["periods"][0]["reset_at"],
            json!("2026-10-24T07:00:00.000Z")
        );
        // RFC3339 strings keep the normalized form.
        assert_eq!(periods[1]["reset_at"], json!("2026-11-02T00:00:00.000Z"));
        // Numeric strings coerce; free reset text passes through.
        assert_eq!(periods[2]["used"], json!(57.5));
        assert_eq!(periods[2]["reset_at"], json!("每月 1 日"));
    }

    #[test]
    fn extractor_can_compute_values() {
        let script = r#"
            #{
                request: #{ url: "https://x.test" },
                extractor: |response| #{
                    used: response.granted - response.left,
                    isValid: response.left > 0,
                    invalidMessage: "已过期"
                }
            }
        "#;
        let data = run_quota_extractor(script, &json!({ "granted": 100, "left": 30.5 }))
            .expect("extractor runs");
        assert_eq!(data["used"], json!(69.5));
        assert_eq!(data["is_valid"], json!(true));
        assert_eq!(data["invalid_message"], json!("已过期"));
    }

    #[test]
    fn extractor_block_body_with_loop_and_periods() {
        // MiniMAX-style extractor: a block body that picks an entry by name and
        // returns the result map via a trailing `let out = ...; out` (a map
        // literal as the last expression of a closure block is a Rhai parse
        // quirk best avoided).
        let script = r#"
            #{
                request: #{
                    url: "https://api.example.com/remains",
                    headers: #{ "Authorization": "Bearer {{apiKey}}" }
                },
                extractor: |response| {
                    let general = response.model_remains[0];
                    for item in response.model_remains {
                        if item.model_name == "general" {
                            general = item;
                        }
                    }
                    let out = #{
                        isValid: response.base_resp.status_code == 0,
                        unit: "%",
                        periods: [
                            #{ key: "5h", used: 100 - general.remain_percent, limit: 100, reset: general.end_time },
                            #{ key: "week", used: 0, limit: 100, reset: general.weekly_end_time }
                        ]
                    };
                    out
                }
            }
        "#;
        let response = json!({
            "base_resp": { "status_code": 0 },
            "model_remains": [
                { "model_name": "video", "remain_percent": 50, "end_time": 1000, "weekly_end_time": 2000 },
                { "model_name": "general", "remain_percent": 91, "end_time": 1792825200000_i64, "weekly_end_time": 1792944000000_i64 }
            ]
        });
        let data = run_quota_extractor(script, &response).expect("extractor runs");
        assert_eq!(data["is_valid"], json!(true));
        let periods = data["periods"].as_array().expect("periods");
        assert_eq!(periods[0]["key"], json!("5h"));
        assert_eq!(periods[0]["used"], json!(9.0));
        assert_eq!(periods[0]["reset_at"], json!("2026-10-24T07:00:00.000Z"));
        assert_eq!(periods[1]["reset_at"], json!("2026-10-25T16:00:00.000Z"));
    }

    #[test]
    fn extractor_errors_are_surfaced() {
        // Missing property in a boolean context raises a Rhai type error.
        let script = "#{ request: #{ url: \"https://x.test\" }, extractor: |r| r.countdown }";
        let error =
            run_quota_extractor(script, &json!({})).expect_err("non-map result should fail");
        assert!(error.contains("extractor"), "unexpected error: {error}");

        let timeout_prone = r#"
            #{
                request: #{ url: "https://x.test" },
                extractor: |r| { let x = 0; loop { x += 1; } #{ remaining: x } }
            }
        "#;
        let error =
            run_quota_extractor(timeout_prone, &json!({})).expect_err("infinite loop must stop");
        assert!(
            error.contains("超时") || error.contains("Too many operations"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn extractor_timeout_kills_infinite_loops() {
        let script = r#"
            #{
                request: #{ url: "https://x.test" },
                extractor: |r| { let x = 0; while true { x += 1; } x }
            }
        "#;
        let error =
            run_quota_extractor(script, &json!({})).expect_err("infinite loop must terminate");
        assert!(
            error.contains("超时") || error.contains("Too many operations"),
            "unexpected: {error}"
        );
    }
}
