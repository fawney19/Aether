use super::payloads::{admin_provider_model_name_exists, build_admin_provider_model_response};
use crate::handlers::admin::provider::shared::paths::admin_provider_models_batch_path;
use crate::handlers::admin::provider::shared::payloads::AdminProviderModelCreateRequest;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS: usize = 100;

fn validate_admin_provider_model_batch(
    payloads: Vec<AdminProviderModelCreateRequest>,
) -> Result<Vec<(String, AdminProviderModelCreateRequest)>, String> {
    if payloads.len() > MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS {
        return Err(format!(
            "批量创建模型最多支持 {MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS} 条"
        ));
    }

    let mut normalized = Vec::with_capacity(payloads.len());
    let mut seen = BTreeSet::new();
    for mut payload in payloads {
        let normalized_name = payload.provider_model_name.trim().to_string();
        if normalized_name.is_empty() {
            return Err("provider_model_name 不能为空".to_string());
        }
        if !seen.insert(normalized_name.clone()) {
            return Err(format!("批量请求中包含重复模型 {normalized_name}"));
        }
        payload.provider_model_name = normalized_name.clone();
        normalized.push((normalized_name, payload));
    }
    Ok(normalized)
}

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() == Some("provider_models_manage")
        && request_context.route_kind() == Some("batch_create_provider_models")
        && request_context.method() == http::Method::POST
        && request_context.path().ends_with("/models/batch")
    {
        let Some(provider_id) = admin_provider_models_batch_path(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payloads =
            match serde_json::from_slice::<Vec<AdminProviderModelCreateRequest>>(request_body) {
                Ok(payloads) => payloads,
                Err(_) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": "请求体必须是合法的 JSON 数组" })),
                        )
                            .into_response(),
                    ));
                }
            };
        let payloads = match validate_admin_provider_model_batch(payloads) {
            Ok(payloads) => payloads,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };

        // Complete request validation before the first write so a bad later item cannot leave
        // an earlier subset committed while the endpoint returns a validation error.
        let mut staged = Vec::new();
        for (normalized_name, payload) in payloads {
            if admin_provider_model_name_exists(state, &provider_id, &normalized_name, None).await?
            {
                continue;
            }
            let record = match state
                .build_admin_provider_model_create_record(&provider_id, payload)
                .await
            {
                Ok(record) => record,
                Err(detail) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": detail })),
                        )
                            .into_response(),
                    ));
                }
            };
            staged.push(record);
        }

        let mut created = Vec::with_capacity(staged.len());
        for record in staged {
            let Some(model) = state.create_admin_provider_model(&record).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "detail": "批量创建模型失败" })),
                    )
                        .into_response(),
                ));
            };
            created.push(model);
        }
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        return Ok(Some(
            Json(serde_json::Value::Array(
                created
                    .iter()
                    .map(|model| {
                        build_admin_provider_model_response(&provider, model, now_unix_secs)
                    })
                    .collect(),
            ))
            .into_response(),
        ));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        validate_admin_provider_model_batch, AdminProviderModelCreateRequest,
        MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS,
    };

    fn payload(name: &str) -> AdminProviderModelCreateRequest {
        serde_json::from_value(serde_json::json!({
            "provider_model_name": name,
            "global_model_id": "global-1"
        }))
        .expect("payload should deserialize")
    }

    #[test]
    fn provider_model_batch_is_bounded_and_prevalidates_duplicates() {
        let at_limit = (0..MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS)
            .map(|index| payload(&format!("model-{index}")))
            .collect();
        assert!(validate_admin_provider_model_batch(at_limit).is_ok());

        let oversized = (0..=MAX_ADMIN_PROVIDER_MODEL_BATCH_ITEMS)
            .map(|index| payload(&format!("model-{index}")))
            .collect();
        assert!(validate_admin_provider_model_batch(oversized).is_err());

        let duplicate = vec![payload("model-1"), payload(" model-1 ")];
        assert!(validate_admin_provider_model_batch(duplicate).is_err());
        assert!(validate_admin_provider_model_batch(vec![payload("   ")]).is_err());
    }
}
