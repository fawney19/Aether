use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const CODEX_ADAPTER_MODULE_NAME: &str = "codex_adapter";
pub const CODEX_ADAPTER_ENABLED_CONFIG_KEY: &str = "module.codex_adapter.enabled";
pub const CODEX_ADAPTER_ROUTES_CONFIG_KEY: &str = "module.codex_adapter.routes";

pub fn normalize_codex_adapter_routes_config_value(value: Value) -> Result<Value, String> {
    let raw_routes = match value {
        Value::Null => return Ok(Value::Array(Vec::new())),
        Value::Array(raw_routes) => raw_routes,
        _ => return Err("codex adapter routes must be an array".to_string()),
    };

    let mut codex_models = BTreeSet::new();
    let mut routes = Vec::with_capacity(raw_routes.len());
    for raw_route in raw_routes {
        let raw_route = raw_route
            .as_object()
            .ok_or_else(|| "codex adapter route must be an object".to_string())?;
        let codex_model = normalize_required_string(raw_route.get("codex_model"), "codex_model")?;
        if !codex_models.insert(codex_model.clone()) {
            return Err(format!("duplicate codex_model: {codex_model}"));
        }

        let enabled = normalize_bool_with_default(raw_route.get("enabled"), true, "enabled")?;
        let scheduling_mode = normalize_scheduling_mode(raw_route.get("scheduling_mode"))?;
        let (candidates, enabled_candidates) = normalize_candidates(raw_route.get("candidates"))?;
        if enabled && enabled_candidates == 0 {
            return Err(format!(
                "enabled codex adapter route '{codex_model}' requires at least one enabled candidate"
            ));
        }

        routes.push(json!({
            "codex_model": codex_model,
            "enabled": enabled,
            "scheduling_mode": scheduling_mode,
            "candidates": candidates,
        }));
    }

    Ok(Value::Array(routes))
}

fn normalize_required_string(value: Option<&Value>, field_name: &str) -> Result<String, String> {
    let raw = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("invalid {field_name}"))?;
    Ok(raw.to_string())
}

fn normalize_bool_with_default(
    value: Option<&Value>,
    default: bool,
    field_name: &str,
) -> Result<bool, String> {
    match value {
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::Null) | None => Ok(default),
        Some(_) => Err(format!("invalid {field_name}")),
    }
}

fn normalize_scheduling_mode(value: Option<&Value>) -> Result<&'static str, String> {
    let raw = normalize_required_string(value, "scheduling_mode")?;
    match raw.as_str() {
        "priority" => Ok("priority"),
        "sticky" => Ok("sticky"),
        "load_balance" => Ok("load_balance"),
        _ => Err(format!("invalid scheduling_mode: {raw}")),
    }
}

fn normalize_candidates(value: Option<&Value>) -> Result<(Value, usize), String> {
    let raw_candidates = value
        .and_then(Value::as_array)
        .ok_or_else(|| "candidates must be an array".to_string())?;
    let mut global_models = BTreeSet::new();
    let mut enabled_candidates = 0usize;
    let mut candidates = Vec::with_capacity(raw_candidates.len());

    for raw_candidate in raw_candidates {
        let raw_candidate = raw_candidate
            .as_object()
            .ok_or_else(|| "candidate must be an object".to_string())?;
        let global_model =
            normalize_required_string(raw_candidate.get("global_model"), "global_model")?;
        if !global_models.insert(global_model.clone()) {
            return Err(format!("duplicate global_model: {global_model}"));
        }

        let enabled =
            normalize_bool_with_default(raw_candidate.get("enabled"), true, "candidate.enabled")?;
        let priority = raw_candidate
            .get("priority")
            .and_then(Value::as_i64)
            .ok_or_else(|| "invalid priority".to_string())?;
        let weight = raw_candidate
            .get("weight")
            .and_then(Value::as_u64)
            .filter(|weight| *weight > 0)
            .ok_or_else(|| "invalid weight".to_string())?;
        if enabled {
            enabled_candidates += 1;
        }

        candidates.push(json!({
            "global_model": global_model,
            "enabled": enabled,
            "priority": priority,
            "weight": weight,
        }));
    }

    Ok((Value::Array(candidates), enabled_candidates))
}

#[cfg(test)]
mod tests {
    use super::normalize_codex_adapter_routes_config_value;

    #[test]
    fn normalizes_valid_codex_adapter_routes() {
        let value = serde_json::json!([
            {
                "codex_model": " gpt-5.5 ",
                "enabled": true,
                "scheduling_mode": "priority",
                "candidates": [
                    { "global_model": " glm-4.6 ", "enabled": true, "priority": 0, "weight": 70 },
                    { "global_model": "deepseek-v3.2", "enabled": true, "priority": 1, "weight": 30 },
                    { "global_model": "disabled-model", "enabled": false, "priority": 2, "weight": 1 }
                ]
            }
        ]);

        let normalized = normalize_codex_adapter_routes_config_value(value).expect("valid routes");
        assert_eq!(normalized[0]["codex_model"], "gpt-5.5");
        assert_eq!(normalized[0]["candidates"][0]["global_model"], "glm-4.6");
        assert_eq!(normalized[0]["candidates"][2]["enabled"], false);
    }

    #[test]
    fn rejects_duplicate_codex_models() {
        let value = serde_json::json!([
            {
                "codex_model": "gpt-5.5",
                "enabled": true,
                "scheduling_mode": "priority",
                "candidates": [{ "global_model": "glm-4.6", "enabled": true, "priority": 0, "weight": 1 }]
            },
            {
                "codex_model": "gpt-5.5",
                "enabled": true,
                "scheduling_mode": "priority",
                "candidates": [{ "global_model": "deepseek-v3.2", "enabled": true, "priority": 0, "weight": 1 }]
            }
        ]);
        assert!(normalize_codex_adapter_routes_config_value(value).is_err());
    }

    #[test]
    fn rejects_invalid_scheduling_mode_empty_enabled_candidates_duplicate_global_model_and_bad_weight(
    ) {
        for (case, value) in [
            (
                "invalid scheduling mode",
                serde_json::json!([{ "codex_model": "gpt-5.5", "enabled": true, "scheduling_mode": "random", "candidates": [{ "global_model": "glm-4.6", "enabled": true, "priority": 0, "weight": 1 }] }]),
            ),
            (
                "empty candidates",
                serde_json::json!([{ "codex_model": "gpt-5.5", "enabled": true, "scheduling_mode": "priority", "candidates": [] }]),
            ),
            (
                "no enabled candidates",
                serde_json::json!([{ "codex_model": "gpt-5.5", "enabled": true, "scheduling_mode": "priority", "candidates": [{ "global_model": "glm-4.6", "enabled": false, "priority": 0, "weight": 1 }] }]),
            ),
            (
                "duplicate global model",
                serde_json::json!([{ "codex_model": "gpt-5.5", "enabled": true, "scheduling_mode": "priority", "candidates": [{ "global_model": "glm-4.6", "enabled": true, "priority": 0, "weight": 1 }, { "global_model": "glm-4.6", "enabled": true, "priority": 1, "weight": 1 }] }]),
            ),
            (
                "zero weight",
                serde_json::json!([{ "codex_model": "gpt-5.5", "enabled": true, "scheduling_mode": "load_balance", "candidates": [{ "global_model": "glm-4.6", "enabled": true, "priority": 0, "weight": 0 }] }]),
            ),
        ] {
            assert!(
                normalize_codex_adapter_routes_config_value(value).is_err(),
                "{case}"
            );
        }
    }
}
