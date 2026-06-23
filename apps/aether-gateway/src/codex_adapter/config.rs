use std::sync::{Arc, RwLock};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexAdapterSchedulingMode {
    Priority,
    Sticky,
    LoadBalance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAdapterRuntimeCandidate {
    pub(crate) global_model: String,
    pub(crate) enabled: bool,
    pub(crate) priority: i64,
    pub(crate) weight: u32,
    pub(crate) order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAdapterRuntimeRoute {
    pub(crate) codex_model: String,
    pub(crate) enabled: bool,
    pub(crate) scheduling_mode: CodexAdapterSchedulingMode,
    pub(crate) candidates: Vec<CodexAdapterRuntimeCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CodexAdapterRuntimeConfig {
    pub(crate) routes: Vec<CodexAdapterRuntimeRoute>,
}

#[derive(Debug, Default)]
pub(crate) struct CodexAdapterRuntimeConfigCache {
    value: RwLock<Option<Arc<CodexAdapterRuntimeConfig>>>,
}

impl CodexAdapterRuntimeConfigCache {
    pub(crate) fn get(&self) -> Option<Arc<CodexAdapterRuntimeConfig>> {
        self.value.read().ok().and_then(|guard| guard.clone())
    }

    pub(crate) fn store(&self, config: Arc<CodexAdapterRuntimeConfig>) {
        if let Ok(mut guard) = self.value.write() {
            *guard = Some(config);
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut guard) = self.value.write() {
            *guard = None;
        }
    }
}

#[derive(Deserialize)]
struct RawRoute {
    codex_model: String,
    enabled: bool,
    scheduling_mode: RawSchedulingMode,
    candidates: Vec<RawCandidate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawSchedulingMode {
    Priority,
    Sticky,
    LoadBalance,
}

#[derive(Deserialize)]
struct RawCandidate {
    global_model: String,
    enabled: bool,
    priority: i64,
    weight: u64,
}

pub(crate) fn parse_codex_adapter_runtime_config(
    value: Value,
) -> Result<CodexAdapterRuntimeConfig, String> {
    let raw_routes: Vec<RawRoute> = serde_json::from_value(value)
        .map_err(|err| format!("invalid codex adapter routes: {err}"))?;
    let routes = raw_routes
        .into_iter()
        .map(parse_runtime_route)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CodexAdapterRuntimeConfig { routes })
}

fn parse_runtime_route(raw: RawRoute) -> Result<CodexAdapterRuntimeRoute, String> {
    let candidates = raw
        .candidates
        .into_iter()
        .enumerate()
        .map(|(order, candidate)| parse_runtime_candidate(order, candidate))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CodexAdapterRuntimeRoute {
        codex_model: raw.codex_model,
        enabled: raw.enabled,
        scheduling_mode: parse_scheduling_mode(raw.scheduling_mode),
        candidates,
    })
}

fn parse_scheduling_mode(raw: RawSchedulingMode) -> CodexAdapterSchedulingMode {
    match raw {
        RawSchedulingMode::Priority => CodexAdapterSchedulingMode::Priority,
        RawSchedulingMode::Sticky => CodexAdapterSchedulingMode::Sticky,
        RawSchedulingMode::LoadBalance => CodexAdapterSchedulingMode::LoadBalance,
    }
}

fn parse_runtime_candidate(
    order: usize,
    raw: RawCandidate,
) -> Result<CodexAdapterRuntimeCandidate, String> {
    let weight = u32::try_from(raw.weight)
        .map_err(|_| format!("codex adapter candidate weight too large: {}", raw.weight))?;
    Ok(CodexAdapterRuntimeCandidate {
        global_model: raw.global_model,
        enabled: raw.enabled,
        priority: raw.priority,
        weight,
        order,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_codex_adapter_runtime_config;

    #[test]
    fn codex_adapter_runtime_config_rejects_candidate_weight_over_u32() {
        let value = serde_json::json!([{
            "codex_model": "gpt-5.5",
            "enabled": true,
            "scheduling_mode": "priority",
            "candidates": [{
                "global_model": "glm-4.6",
                "enabled": true,
                "priority": 0,
                "weight": u64::from(u32::MAX) + 1
            }]
        }]);

        let err = parse_codex_adapter_runtime_config(value)
            .expect_err("weight larger than u32 should fail");

        assert!(err.contains("weight too large"));
    }
}
