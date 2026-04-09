use super::super::{
    admin_billing_validate_safe_expression, default_admin_billing_true,
    normalize_admin_billing_optional_text, normalize_admin_billing_required_text,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

fn default_admin_billing_collector_value_type() -> String {
    "float".to_string()
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminBillingCollectorUpsertRequest {
    pub(super) api_format: String,
    pub(super) task_type: String,
    pub(super) dimension_name: String,
    pub(super) source_type: String,
    #[serde(default)]
    pub(super) source_path: Option<String>,
    #[serde(default = "default_admin_billing_collector_value_type")]
    pub(super) value_type: String,
    #[serde(default)]
    pub(super) transform_expression: Option<String>,
    #[serde(default)]
    pub(super) default_value: Option<String>,
    #[serde(default)]
    pub(super) priority: i32,
    #[serde(default = "default_admin_billing_true")]
    pub(super) is_enabled: bool,
}

pub(super) fn build_admin_billing_collector_payload_from_record(
    record: &crate::AdminBillingCollectorRecord,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "api_format": record.api_format,
        "task_type": record.task_type,
        "dimension_name": record.dimension_name,
        "source_type": record.source_type,
        "source_path": record.source_path,
        "value_type": record.value_type,
        "transform_expression": record.transform_expression,
        "default_value": record.default_value,
        "priority": record.priority,
        "is_enabled": record.is_enabled,
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "updated_at": unix_secs_to_rfc3339(record.updated_at_unix_secs),
    })
}

pub(super) fn admin_billing_collector_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/billing/collectors/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

pub(super) async fn parse_admin_billing_collector_request(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
    existing_id: Option<&str>,
) -> Result<crate::AdminBillingCollectorWriteInput, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_admin_billing_bad_request_response("请求体不能为空"));
    };
    let request = match serde_json::from_slice::<AdminBillingCollectorUpsertRequest>(request_body) {
        Ok(value) => value,
        Err(err) => {
            return Err(build_admin_billing_bad_request_response(format!(
                "Invalid request body: {err}"
            )));
        }
    };

    let api_format =
        match normalize_admin_billing_required_text(&request.api_format, "api_format", 50) {
            Ok(value) => value.to_ascii_uppercase(),
            Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
        };
    let task_type = match normalize_admin_billing_required_text(&request.task_type, "task_type", 20)
    {
        Ok(value) => value.to_ascii_lowercase(),
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    let dimension_name =
        match normalize_admin_billing_required_text(&request.dimension_name, "dimension_name", 100)
        {
            Ok(value) => value,
            Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
        };
    let source_type = request.source_type.trim().to_ascii_lowercase();
    if !matches!(
        source_type.as_str(),
        "request" | "response" | "metadata" | "computed"
    ) {
        return Err(build_admin_billing_bad_request_response(
            "source_type must be one of request, response, metadata, computed",
        ));
    }
    let value_type = request.value_type.trim().to_ascii_lowercase();
    if !matches!(value_type.as_str(), "float" | "int" | "string") {
        return Err(build_admin_billing_bad_request_response(
            "value_type must be one of float, int, string",
        ));
    }
    let source_path = match normalize_admin_billing_optional_text(request.source_path, 200) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    let transform_expression =
        match normalize_admin_billing_optional_text(request.transform_expression, 4096) {
            Ok(value) => value,
            Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
        };
    let default_value = match normalize_admin_billing_optional_text(request.default_value, 100) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };

    if source_type == "computed" {
        if source_path.is_some() {
            return Err(build_admin_billing_bad_request_response(
                "computed collector must have source_path=null",
            ));
        }
        if transform_expression.is_none() {
            return Err(build_admin_billing_bad_request_response(
                "computed collector must have transform_expression",
            ));
        }
    } else if source_path.is_none() {
        return Err(build_admin_billing_bad_request_response(
            "non-computed collector must have source_path",
        ));
    }

    if let Some(transform_expression) = transform_expression.as_deref() {
        if let Err(detail) = admin_billing_validate_safe_expression(transform_expression) {
            return Err(build_admin_billing_bad_request_response(format!(
                "Invalid transform_expression: {detail}"
            )));
        }
    }

    if default_value.is_some() && request.is_enabled {
        match state
            .admin_billing_enabled_default_value_exists(
                &api_format,
                &task_type,
                &dimension_name,
                existing_id,
            )
            .await
        {
            Ok(true) => {
                return Err(build_admin_billing_bad_request_response(
                    "default_value already exists for this (api_format, task_type, dimension_name)",
                ));
            }
            Ok(false) => {}
            Err(err) => {
                let detail = match err {
                    GatewayError::Internal(message) => message,
                    other => format!("{other:?}"),
                };
                return Err((
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        }
    }

    Ok(crate::AdminBillingCollectorWriteInput {
        api_format,
        task_type,
        dimension_name,
        source_type,
        source_path,
        value_type,
        transform_expression,
        default_value,
        priority: request.priority,
        is_enabled: request.is_enabled,
    })
}

pub(in super::super) fn admin_billing_parse_page(query: Option<&str>) -> Result<u32, String> {
    super::super::admin_billing_parse_page(query)
}

pub(in super::super) fn admin_billing_parse_page_size(query: Option<&str>) -> Result<u32, String> {
    super::super::admin_billing_parse_page_size(query)
}

pub(in super::super) fn admin_billing_optional_filter(
    query: Option<&str>,
    key: &str,
) -> Option<String> {
    super::super::admin_billing_optional_filter(query, key)
}

pub(in super::super) fn admin_billing_optional_bool_filter(
    query: Option<&str>,
    key: &str,
) -> Result<Option<bool>, String> {
    super::super::admin_billing_optional_bool_filter(query, key)
}

pub(in super::super) fn admin_billing_pages(total: u64, page_size: u32) -> u64 {
    super::super::admin_billing_pages(total, page_size)
}

pub(in super::super) fn build_admin_billing_bad_request_response(
    detail: impl Into<String>,
) -> Response<Body> {
    super::super::build_admin_billing_bad_request_response(detail)
}

pub(in super::super) fn build_admin_billing_not_found_response(
    detail: &'static str,
) -> Response<Body> {
    super::super::build_admin_billing_not_found_response(detail)
}

pub(in super::super) fn build_admin_billing_read_only_response(
    detail: &'static str,
) -> Response<Body> {
    super::super::build_admin_billing_read_only_response(detail)
}
