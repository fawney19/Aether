use super::{
    admin_billing_optional_bool_filter, admin_billing_optional_filter, admin_billing_pages,
    admin_billing_parse_page, admin_billing_parse_page_size,
    admin_billing_validate_safe_expression, build_admin_billing_bad_request_response,
    build_admin_billing_not_found_response, build_admin_billing_read_only_response,
    default_admin_billing_json_object, default_admin_billing_true,
    normalize_admin_billing_optional_text, normalize_admin_billing_required_text,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
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

fn default_admin_billing_rule_task_type() -> String {
    "chat".to_string()
}

#[derive(Debug, Deserialize)]
struct AdminBillingRuleUpsertRequest {
    name: String,
    #[serde(default = "default_admin_billing_rule_task_type")]
    task_type: String,
    #[serde(default)]
    global_model_id: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    expression: String,
    #[serde(default = "default_admin_billing_json_object")]
    variables: serde_json::Value,
    #[serde(default = "default_admin_billing_json_object")]
    dimension_mappings: serde_json::Value,
    #[serde(default = "default_admin_billing_true")]
    is_enabled: bool,
}

fn build_admin_billing_rule_payload_from_record(
    record: &crate::AdminBillingRuleRecord,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "name": record.name,
        "task_type": record.task_type,
        "global_model_id": record.global_model_id,
        "model_id": record.model_id,
        "expression": record.expression,
        "variables": record.variables,
        "dimension_mappings": record.dimension_mappings,
        "is_enabled": record.is_enabled,
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "updated_at": unix_secs_to_rfc3339(record.updated_at_unix_secs),
    })
}

fn admin_billing_rule_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/billing/rules/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn parse_admin_billing_rule_request(
    request_body: Option<&Bytes>,
) -> Result<crate::AdminBillingRuleWriteInput, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_admin_billing_bad_request_response("请求体不能为空"));
    };
    let request = match serde_json::from_slice::<AdminBillingRuleUpsertRequest>(request_body) {
        Ok(value) => value,
        Err(err) => {
            return Err(build_admin_billing_bad_request_response(format!(
                "Invalid request body: {err}"
            )))
        }
    };

    let name = match normalize_admin_billing_required_text(&request.name, "name", 100) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    let task_type = request.task_type.trim().to_ascii_lowercase();
    if !matches!(task_type.as_str(), "chat" | "video" | "image" | "audio") {
        return Err(build_admin_billing_bad_request_response(
            "task_type must be one of chat, video, image, audio",
        ));
    }
    let global_model_id = match normalize_admin_billing_optional_text(request.global_model_id, 64) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    let model_id = match normalize_admin_billing_optional_text(request.model_id, 64) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    if global_model_id.is_some() == model_id.is_some() {
        return Err(build_admin_billing_bad_request_response(
            "Exactly one of global_model_id or model_id must be provided",
        ));
    }
    let expression = request.expression.trim().to_string();
    if let Err(detail) = admin_billing_validate_safe_expression(&expression) {
        return Err(build_admin_billing_bad_request_response(format!(
            "Invalid expression: {detail}"
        )));
    }

    let Some(variables) = request.variables.as_object() else {
        return Err(build_admin_billing_bad_request_response(
            "variables must be a JSON object",
        ));
    };
    for (key, value) in variables {
        if key.trim().is_empty() {
            return Err(build_admin_billing_bad_request_response(
                "variables keys must be non-empty strings",
            ));
        }
        if value.is_boolean() || !value.is_number() {
            return Err(build_admin_billing_bad_request_response(format!(
                "variables['{key}'] must be a number"
            )));
        }
    }

    let Some(dimension_mappings) = request.dimension_mappings.as_object() else {
        return Err(build_admin_billing_bad_request_response(
            "dimension_mappings must be a JSON object",
        ));
    };
    for (key, value) in dimension_mappings {
        if key.trim().is_empty() {
            return Err(build_admin_billing_bad_request_response(
                "dimension_mappings keys must be non-empty strings",
            ));
        }
        let Some(mapping) = value.as_object() else {
            return Err(build_admin_billing_bad_request_response(format!(
                "dimension_mappings['{key}'] must be an object"
            )));
        };
        if !mapping.contains_key("source") {
            return Err(build_admin_billing_bad_request_response(format!(
                "dimension_mappings['{key}'].source is required"
            )));
        }
    }

    Ok(crate::AdminBillingRuleWriteInput {
        name,
        task_type,
        global_model_id,
        model_id,
        expression,
        variables: serde_json::Value::Object(variables.clone()),
        dimension_mappings: serde_json::Value::Object(dimension_mappings.clone()),
        is_enabled: request.is_enabled,
    })
}

async fn build_admin_list_billing_rules_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let page = match admin_billing_parse_page(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let page_size = match admin_billing_parse_page_size(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let task_type = admin_billing_optional_filter(query, "task_type");
    let is_enabled = match admin_billing_optional_bool_filter(query, "is_enabled") {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };

    let (items, total) = if let Some((records, record_total)) = state
        .list_admin_billing_rules(task_type.as_deref(), is_enabled, page, page_size)
        .await?
    {
        (
            records
                .iter()
                .map(build_admin_billing_rule_payload_from_record)
                .collect::<Vec<_>>(),
            record_total,
        )
    } else {
        (Vec::new(), 0)
    };

    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": admin_billing_pages(total, page_size),
    }))
    .into_response())
}

async fn build_admin_get_billing_rule_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(rule_id) = admin_billing_rule_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response("缺少 rule_id"));
    };
    match state.read_admin_billing_rule(&rule_id).await? {
        Some(record) => {
            Ok(Json(build_admin_billing_rule_payload_from_record(&record)).into_response())
        }
        None => Ok(build_admin_billing_not_found_response(
            "Billing rule not found",
        )),
    }
}

async fn build_admin_create_billing_rule_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let input = match parse_admin_billing_rule_request(request_body) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    match state.create_admin_billing_rule(&input).await? {
        crate::LocalMutationOutcome::Applied(record) => {
            Ok(Json(build_admin_billing_rule_payload_from_record(&record)).into_response())
        }
        crate::LocalMutationOutcome::Invalid(detail) => {
            Ok(build_admin_billing_bad_request_response(detail))
        }
        crate::LocalMutationOutcome::NotFound => Ok(build_admin_billing_not_found_response(
            "Billing rule not found",
        )),
        crate::LocalMutationOutcome::Unavailable => Ok(build_admin_billing_read_only_response(
            "当前为只读模式，无法创建计费规则",
        )),
    }
}

async fn build_admin_update_billing_rule_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(rule_id) = admin_billing_rule_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response("缺少 rule_id"));
    };
    let input = match parse_admin_billing_rule_request(request_body) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    match state.update_admin_billing_rule(&rule_id, &input).await? {
        crate::LocalMutationOutcome::Applied(record) => {
            Ok(Json(build_admin_billing_rule_payload_from_record(&record)).into_response())
        }
        crate::LocalMutationOutcome::NotFound => Ok(build_admin_billing_not_found_response(
            "Billing rule not found",
        )),
        crate::LocalMutationOutcome::Invalid(detail) => {
            Ok(build_admin_billing_bad_request_response(detail))
        }
        crate::LocalMutationOutcome::Unavailable => Ok(build_admin_billing_read_only_response(
            "当前为只读模式，无法更新计费规则",
        )),
    }
}

pub(super) async fn maybe_build_local_admin_billing_rules_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    let path = request_context.path();

    match decision.route_kind.as_deref() {
        Some("list_rules")
            if request_context.method() == http::Method::GET
                && matches!(
                    path,
                    "/api/admin/billing/rules" | "/api/admin/billing/rules/"
                ) =>
        {
            Ok(Some(
                build_admin_list_billing_rules_response(state, request_context).await?,
            ))
        }
        Some("get_rule")
            if request_context.method() == http::Method::GET
                && path.starts_with("/api/admin/billing/rules/") =>
        {
            Ok(Some(
                build_admin_get_billing_rule_response(state, request_context).await?,
            ))
        }
        Some("create_rule")
            if request_context.method() == http::Method::POST
                && matches!(
                    path,
                    "/api/admin/billing/rules" | "/api/admin/billing/rules/"
                ) =>
        {
            Ok(Some(
                build_admin_create_billing_rule_response(state, request_body).await?,
            ))
        }
        Some("update_rule")
            if request_context.method() == http::Method::PUT
                && path.starts_with("/api/admin/billing/rules/") =>
        {
            Ok(Some(
                build_admin_update_billing_rule_response(state, request_context, request_body)
                    .await?,
            ))
        }
        _ => Ok(None),
    }
}
