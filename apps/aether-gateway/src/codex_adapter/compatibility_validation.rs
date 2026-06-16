use std::collections::BTreeMap;

use crate::codex_adapter::compatibility::{
    codex_adapter_compatibility_summary, read_codex_adapter_global_model_compatibilities,
};
use crate::codex_adapter::config::{parse_codex_adapter_runtime_config, CodexAdapterRuntimeRoute};
use crate::{AppState, GatewayError};

fn codex_adapter_bad_request(message: String) -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::BAD_REQUEST,
        message,
    }
}

pub(crate) async fn validate_codex_adapter_routes_config_write(
    state: &AppState,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), GatewayError> {
    if key.trim() != aether_admin::codex_adapter::CODEX_ADAPTER_ROUTES_CONFIG_KEY {
        return Ok(());
    }
    if !state.has_provider_catalog_data_reader() || !state.has_minimal_candidate_selection_reader()
    {
        return Ok(());
    }

    let normalized =
        aether_admin::codex_adapter::normalize_codex_adapter_routes_config_value(value.clone())
            .map_err(codex_adapter_bad_request)?;
    let config =
        parse_codex_adapter_runtime_config(normalized).map_err(codex_adapter_bad_request)?;
    let enabled_models = collect_enabled_candidate_models(&config.routes);
    if enabled_models.is_empty() {
        return Ok(());
    }

    let compatibilities =
        read_codex_adapter_global_model_compatibilities(state, &enabled_models).await?;
    let compatibility_map = compatibilities
        .into_iter()
        .map(|item| (item.global_model.clone(), item))
        .collect::<BTreeMap<_, _>>();

    for route in &config.routes {
        if !route.enabled {
            continue;
        }
        for candidate in &route.candidates {
            if !candidate.enabled {
                continue;
            }
            let Some(compatibility) = compatibility_map.get(candidate.global_model.as_str()) else {
                continue;
            };
            if compatibility.compatible {
                continue;
            }
            return Err(codex_adapter_bad_request(format!(
                "Codex 路由 '{}' 的候选模型 '{}' 当前不可用于 Responses：{}",
                route.codex_model,
                candidate.global_model,
                codex_adapter_compatibility_summary(compatibility),
            )));
        }
    }

    Ok(())
}

fn collect_enabled_candidate_models(routes: &[CodexAdapterRuntimeRoute]) -> Vec<String> {
    let mut models = Vec::new();
    for route in routes {
        if !route.enabled {
            continue;
        }
        for candidate in &route.candidates {
            if candidate.enabled {
                models.push(candidate.global_model.clone());
            }
        }
    }
    models
}
