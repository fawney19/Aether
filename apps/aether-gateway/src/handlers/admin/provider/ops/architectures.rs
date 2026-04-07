use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_ops_architecture_id_from_path, is_admin_provider_ops_architectures_root,
};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

static ADMIN_PROVIDER_OPS_ARCHITECTURES_ALL: std::sync::LazyLock<Vec<Value>> =
    std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("architectures.all.json"))
            .expect("admin provider ops architectures fixture should parse")
    });

fn admin_provider_ops_architectures_list_payload() -> Vec<Value> {
    ADMIN_PROVIDER_OPS_ARCHITECTURES_ALL
        .iter()
        .filter(|item| item.get("architecture_id").and_then(Value::as_str) != Some("generic_api"))
        .cloned()
        .collect()
}

fn admin_provider_ops_architecture_payload(architecture_id: &str) -> Option<Value> {
    ADMIN_PROVIDER_OPS_ARCHITECTURES_ALL
        .iter()
        .find_map(|item| {
            (item.get("architecture_id").and_then(Value::as_str) == Some(architecture_id))
                .then(|| item.clone())
        })
}

pub(super) async fn maybe_build_local_admin_provider_ops_architectures_response(
    request_context: &GatewayPublicRequestContext,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("provider_ops_manage")
        && decision.route_kind.as_deref() == Some("list_architectures")
        && request_context.request_method == http::Method::GET
        && is_admin_provider_ops_architectures_root(&request_context.request_path)
    {
        return Ok(Some(
            Json(admin_provider_ops_architectures_list_payload()).into_response(),
        ));
    }

    if decision.route_family.as_deref() == Some("provider_ops_manage")
        && decision.route_kind.as_deref() == Some("get_architecture")
        && request_context.request_method == http::Method::GET
    {
        let Some(architecture_id) =
            admin_provider_ops_architecture_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "架构不存在" })),
                )
                    .into_response(),
            ));
        };

        return Ok(Some(
            match admin_provider_ops_architecture_payload(&architecture_id) {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("架构 {architecture_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
