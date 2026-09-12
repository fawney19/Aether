use std::collections::BTreeMap;

use aether_data_contracts::repository::routing_profiles::{
    CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, CreateRoutingGroupVersionRecord,
    RoutingGroupBindingQuery, RoutingGroupBindingSubject, RoutingGroupLookupKey,
    StoredRoutingGroup, StoredRoutingGroupBinding, StoredRoutingGroupVersion,
    UpdateRoutingGroupBindingRecord, UpdateRoutingGroupRecord,
};
use aether_routing_core::{
    validate_routing_group_config, MutationPlan, RoutingGroupConfig, RoutingHeaderPatch,
    RoutingPatchSummary, RoutingRulePhase,
};
use axum::{
    body::{Body, Bytes},
    http::{self, HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::clock::current_unix_secs;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::routing::{
    apply_routing_mutation_plan, build_routing_trace_seed, resolve_gateway_routing_policy,
    GatewayRoutingPolicyInput,
};
use crate::GatewayError;

const ROUTING_GROUPS_ROOT: &str = "/api/admin/routing/groups";
const ROUTING_BINDINGS_ROOT: &str = "/api/admin/routing/bindings";

#[derive(Debug, Deserialize)]
struct AdminRoutingGroupCreateRequest {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    is_system_default: bool,
    #[serde(default)]
    sort_order: i64,
    #[serde(default)]
    config_json: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AdminRoutingGroupBindingCreateRequest {
    #[serde(default)]
    id: Option<String>,
    group_id: String,
    subject_type: RoutingGroupBindingSubject,
    subject_id: String,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    allow_explicit_select: bool,
}

#[derive(Debug, Deserialize)]
struct AdminRoutingDryRunRequest {
    model: String,
    #[serde(default)]
    resolved_model: Option<String>,
    #[serde(default = "default_api_format")]
    api_format: String,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    api_key_id: Option<String>,
    #[serde(default)]
    headers: Option<Value>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    phase: Option<RoutingRulePhase>,
}

pub(crate) async fn maybe_build_local_admin_routing_response(
    request: crate::handlers::admin::request::AdminRouteRequest<'_>,
) -> crate::handlers::admin::request::AdminRouteResult {
    let state = request.state();
    let request_context = request.request_context();
    let request_body = request.request_body();

    if request_context.route_family() != Some("routing_profiles_manage") {
        return Ok(None);
    }
    if !request_context.path().starts_with("/api/admin/routing/") {
        return Ok(None);
    }
    if !state.has_routing_group_data_reader() {
        return Ok(Some(data_unavailable_response()));
    }

    let response = if request_context.path().starts_with(ROUTING_GROUPS_ROOT) {
        maybe_build_routing_groups_response(&state, &request_context, request_body).await?
    } else if request_context.path().starts_with(ROUTING_BINDINGS_ROOT) {
        maybe_build_routing_bindings_response(&state, &request_context, request_body).await?
    } else {
        None
    };
    Ok(response)
}

async fn maybe_build_routing_groups_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let path = normalized_admin_path(request_context.path());
    match (request_context.method(), path.as_str()) {
        (&http::Method::GET, ROUTING_GROUPS_ROOT) => {
            let groups = state.list_routing_groups().await?;
            Ok(Some(
                Json(json!({
                    "items": groups.iter().map(routing_group_payload).collect::<Vec<_>>(),
                    "total": groups.len(),
                }))
                .into_response(),
            ))
        }
        (&http::Method::POST, ROUTING_GROUPS_ROOT) => {
            if !state.has_routing_group_data_writer() {
                return Ok(Some(data_unavailable_response()));
            }
            let payload = parse_json_body::<AdminRoutingGroupCreateRequest>(request_body)?;
            let config_json = payload.config_json.unwrap_or_else(|| json!({}));
            validate_config_json(&config_json)?;
            let now = current_unix_secs() as i64;
            let record = CreateRoutingGroupRecord {
                id: payload.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
                name: payload.name,
                description: payload.description,
                enabled: payload.enabled,
                is_system_default: payload.is_system_default,
                sort_order: payload.sort_order,
                config_json,
                version: 1,
                created_at: now,
                updated_at: now,
                published_at: None,
            };
            let Some(created) = state.create_routing_group(record).await? else {
                return Ok(Some(data_unavailable_response()));
            };
            Ok(Some(attach_admin_audit_response(
                Json(routing_group_payload(&created)).into_response(),
                "admin_routing_group_created",
                "create_routing_group",
                "routing_group",
                &created.id,
            )))
        }
        _ => {
            let Some((group_id, suffix)) = routing_group_path_parts(path.as_str()) else {
                return Ok(None);
            };
            match (request_context.method(), suffix.as_deref()) {
                (&http::Method::GET, None) => {
                    let Some(group) = state
                        .find_routing_group(RoutingGroupLookupKey::Id(&group_id))
                        .await?
                    else {
                        return Ok(Some(not_found_response(format!(
                            "routing group {group_id} not found"
                        ))));
                    };
                    Ok(Some(Json(routing_group_payload(&group)).into_response()))
                }
                (&http::Method::PATCH, None) => {
                    if !state.has_routing_group_data_writer() {
                        return Ok(Some(data_unavailable_response()));
                    }
                    let patch = build_routing_group_update_patch(request_body)?;
                    let Some(updated) = state.update_routing_group(&group_id, patch).await? else {
                        return Ok(Some(not_found_response(format!(
                            "routing group {group_id} not found"
                        ))));
                    };
                    Ok(Some(attach_admin_audit_response(
                        Json(routing_group_payload(&updated)).into_response(),
                        "admin_routing_group_updated",
                        "update_routing_group",
                        "routing_group",
                        &updated.id,
                    )))
                }
                (&http::Method::DELETE, None) => {
                    if !state.has_routing_group_data_writer() {
                        return Ok(Some(data_unavailable_response()));
                    }
                    if !state.delete_routing_group(&group_id).await? {
                        return Ok(Some(not_found_response(format!(
                            "routing group {group_id} not found"
                        ))));
                    }
                    Ok(Some(attach_admin_audit_response(
                        http::StatusCode::NO_CONTENT.into_response(),
                        "admin_routing_group_deleted",
                        "delete_routing_group",
                        "routing_group",
                        &group_id,
                    )))
                }
                (&http::Method::POST, Some("publish")) => {
                    publish_routing_group(state, &group_id).await
                }
                (&http::Method::GET, Some("versions")) => {
                    let versions = state.list_routing_group_versions(&group_id).await?;
                    Ok(Some(Json(json!({
                        "items": versions.iter().map(routing_group_version_payload).collect::<Vec<_>>(),
                        "total": versions.len(),
                    })).into_response()))
                }
                (&http::Method::POST, Some("dry-run")) => {
                    dry_run_routing_group(state, &group_id, request_body).await
                }
                _ => Ok(None),
            }
        }
    }
}

async fn maybe_build_routing_bindings_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let path = normalized_admin_path(request_context.path());
    match (request_context.method(), path.as_str()) {
        (&http::Method::GET, ROUTING_BINDINGS_ROOT) => {
            let query = routing_binding_query_from_request(request_context)?;
            let bindings = state.list_routing_group_bindings(&query).await?;
            Ok(Some(
                Json(json!({
                    "items": bindings.iter().map(routing_group_binding_payload).collect::<Vec<_>>(),
                    "total": bindings.len(),
                }))
                .into_response(),
            ))
        }
        (&http::Method::POST, ROUTING_BINDINGS_ROOT) => {
            if !state.has_routing_group_data_writer() {
                return Ok(Some(data_unavailable_response()));
            }
            let payload = parse_json_body::<AdminRoutingGroupBindingCreateRequest>(request_body)?;
            let now = current_unix_secs() as i64;
            let record = CreateRoutingGroupBindingRecord {
                id: payload.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
                group_id: payload.group_id,
                subject_type: payload.subject_type,
                subject_id: payload.subject_id,
                is_default: payload.is_default,
                allow_explicit_select: payload.allow_explicit_select,
                created_at: now,
                updated_at: now,
            };
            let Some(created) = state.create_routing_group_binding(record).await? else {
                return Ok(Some(data_unavailable_response()));
            };
            Ok(Some(attach_admin_audit_response(
                Json(routing_group_binding_payload(&created)).into_response(),
                "admin_routing_group_binding_created",
                "create_routing_group_binding",
                "routing_group_binding",
                &created.id,
            )))
        }
        _ => {
            let Some(binding_id) = routing_binding_id_from_path(path.as_str()) else {
                return Ok(None);
            };
            match *request_context.method() {
                http::Method::PATCH => {
                    if !state.has_routing_group_data_writer() {
                        return Ok(Some(data_unavailable_response()));
                    }
                    let patch = build_routing_binding_update_patch(request_body)?;
                    let Some(updated) = state
                        .update_routing_group_binding(&binding_id, patch)
                        .await?
                    else {
                        return Ok(Some(not_found_response(format!(
                            "routing group binding {binding_id} not found"
                        ))));
                    };
                    Ok(Some(attach_admin_audit_response(
                        Json(routing_group_binding_payload(&updated)).into_response(),
                        "admin_routing_group_binding_updated",
                        "update_routing_group_binding",
                        "routing_group_binding",
                        &updated.id,
                    )))
                }
                http::Method::DELETE => {
                    if !state.has_routing_group_data_writer() {
                        return Ok(Some(data_unavailable_response()));
                    }
                    if !state.delete_routing_group_binding(&binding_id).await? {
                        return Ok(Some(not_found_response(format!(
                            "routing group binding {binding_id} not found"
                        ))));
                    }
                    Ok(Some(attach_admin_audit_response(
                        http::StatusCode::NO_CONTENT.into_response(),
                        "admin_routing_group_binding_deleted",
                        "delete_routing_group_binding",
                        "routing_group_binding",
                        &binding_id,
                    )))
                }
                _ => Ok(None),
            }
        }
    }
}

async fn publish_routing_group(
    state: &AdminAppState<'_>,
    group_id: &str,
) -> Result<Option<Response<Body>>, GatewayError> {
    if !state.has_routing_group_data_writer() {
        return Ok(Some(data_unavailable_response()));
    }
    let Some(group) = state
        .find_routing_group(RoutingGroupLookupKey::Id(group_id))
        .await?
    else {
        return Ok(Some(not_found_response(format!(
            "routing group {group_id} not found"
        ))));
    };
    validate_config_json(&group.config_json)?;
    let latest_version = state
        .list_routing_group_versions(group_id)
        .await?
        .into_iter()
        .map(|version| version.version)
        .max()
        .unwrap_or(0);
    let next_version = group.version.max(latest_version.saturating_add(1));
    let now = current_unix_secs() as i64;
    let Some(updated) = state
        .update_routing_group(
            group_id,
            UpdateRoutingGroupRecord {
                version: Some(next_version),
                updated_at: now,
                published_at: Some(Some(now)),
                ..UpdateRoutingGroupRecord::default()
            },
        )
        .await?
    else {
        return Ok(Some(not_found_response(format!(
            "routing group {group_id} not found"
        ))));
    };
    let _ = state
        .create_routing_group_version(CreateRoutingGroupVersionRecord {
            id: Uuid::new_v4().to_string(),
            group_id: group_id.to_string(),
            version: next_version,
            config_json: updated.config_json.clone(),
            created_at: now,
            created_by: None,
        })
        .await?;

    Ok(Some(attach_admin_audit_response(
        Json(routing_group_payload(&updated)).into_response(),
        "admin_routing_group_published",
        "publish_routing_group",
        "routing_group",
        group_id,
    )))
}

async fn dry_run_routing_group(
    state: &AdminAppState<'_>,
    group_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(group) = state
        .find_routing_group(RoutingGroupLookupKey::Id(group_id))
        .await?
    else {
        return Ok(Some(not_found_response(format!(
            "routing group {group_id} not found"
        ))));
    };
    let payload = parse_json_body::<AdminRoutingDryRunRequest>(request_body)?;
    let requested_model = payload.model.trim();
    if requested_model.is_empty() {
        return Ok(Some(bad_request_response("model must not be empty")));
    }
    let resolved_model = payload
        .resolved_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(requested_model);
    let api_format = payload.api_format.trim();
    let headers_json = payload.headers.unwrap_or_else(|| json!({}));
    let mut header_map = header_map_from_value(&headers_json)?;
    let mut body = payload.body.unwrap_or_else(|| json!({}));
    let policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
        group_id: Some(group.id.as_str()),
        group_version: Some(group.version),
        group_config_json: &group.config_json,
        selection_source: "admin_dry_run",
        requested_model,
        resolved_model,
        api_format,
        user_id: payload.user_id.as_deref(),
        api_key_id: payload.api_key_id.as_deref(),
        headers: &headers_json,
        body: &body,
        phase: payload.phase.unwrap_or(RoutingRulePhase::ClientRequest),
    })?;
    let patch_summary = patch_summary(&policy.mutation_plan);
    apply_routing_mutation_plan(&mut body, &mut header_map, &policy.mutation_plan)?;
    let mut trace = build_routing_trace_seed(&policy, api_format);
    trace.client_request_patch_summary = patch_summary.clone();

    Ok(Some(Json(json!({
        "group": routing_group_payload(&group),
        "policy": policy,
        "trace_seed": trace,
        "patch_summary": patch_summary,
        "mutated_body": body,
        "mutated_headers": header_map_payload(&header_map),
        "candidate_preview": {
            "status": "policy_only",
            "ranking_overlay": policy.ranking_overlay,
            "note": "full candidate preview is produced by runtime materialization once provider/key catalogs are enumerated"
        }
    })).into_response()))
}

fn build_routing_group_update_patch(
    request_body: Option<&Bytes>,
) -> Result<UpdateRoutingGroupRecord, GatewayError> {
    let raw = parse_json_value_body(request_body)?;
    let Some(object) = raw.as_object() else {
        return Err(bad_request_error("request body must be a JSON object"));
    };
    let mut patch = UpdateRoutingGroupRecord {
        updated_at: current_unix_secs() as i64,
        ..UpdateRoutingGroupRecord::default()
    };
    if let Some(value) = object.get("name") {
        patch.name = Some(required_string(value, "name")?);
    }
    if let Some(value) = object.get("description") {
        patch.description = Some(optional_string(value, "description")?);
    }
    if let Some(value) = object.get("enabled") {
        patch.enabled = Some(required_bool(value, "enabled")?);
    }
    if let Some(value) = object.get("is_system_default") {
        patch.is_system_default = Some(required_bool(value, "is_system_default")?);
    }
    if let Some(value) = object.get("sort_order") {
        patch.sort_order = Some(required_i64(value, "sort_order")?.max(0));
    }
    if let Some(value) = object.get("config_json") {
        validate_config_json(value)?;
        patch.config_json = Some(value.clone());
        patch.version = object
            .get("version")
            .and_then(Value::as_i64)
            .or(Some(current_unix_secs() as i64));
    } else if let Some(value) = object.get("version") {
        patch.version = Some(required_i64(value, "version")?.max(1));
    }
    if let Some(value) = object.get("published_at") {
        patch.published_at = Some(optional_i64(value, "published_at")?);
    }
    Ok(patch)
}

fn build_routing_binding_update_patch(
    request_body: Option<&Bytes>,
) -> Result<UpdateRoutingGroupBindingRecord, GatewayError> {
    let raw = parse_json_value_body(request_body)?;
    let Some(object) = raw.as_object() else {
        return Err(bad_request_error("request body must be a JSON object"));
    };
    let mut patch = UpdateRoutingGroupBindingRecord {
        updated_at: current_unix_secs() as i64,
        ..UpdateRoutingGroupBindingRecord::default()
    };
    if let Some(value) = object.get("group_id") {
        patch.group_id = Some(required_string(value, "group_id")?);
    }
    if let Some(value) = object.get("subject_type") {
        patch.subject_type = Some(routing_subject_from_value(value)?);
    }
    if let Some(value) = object.get("subject_id") {
        patch.subject_id = Some(required_string(value, "subject_id")?);
    }
    if let Some(value) = object.get("is_default") {
        patch.is_default = Some(required_bool(value, "is_default")?);
    }
    if let Some(value) = object.get("allow_explicit_select") {
        patch.allow_explicit_select = Some(required_bool(value, "allow_explicit_select")?);
    }
    Ok(patch)
}

fn routing_binding_query_from_request(
    request_context: &AdminRequestContext<'_>,
) -> Result<RoutingGroupBindingQuery, GatewayError> {
    let subject_type = query_param_value(request_context.query_string(), "subject_type")
        .map(|value| routing_subject_from_str(&value))
        .transpose()?;
    Ok(RoutingGroupBindingQuery {
        group_id: query_param_value(request_context.query_string(), "group_id"),
        subject_type,
        subject_id: query_param_value(request_context.query_string(), "subject_id"),
    })
}

fn validate_config_json(value: &Value) -> Result<(), GatewayError> {
    if !value.is_object() {
        return Err(bad_request_error("config_json must be a JSON object"));
    }
    let config = serde_json::from_value::<RoutingGroupConfig>(value.clone())
        .map_err(|err| bad_request_error(format!("config_json is invalid: {err}")))?;
    validate_routing_group_config(&config)
        .map_err(|err| bad_request_error(format!("config_json is invalid: {err}")))
}

fn parse_json_body<T>(request_body: Option<&Bytes>) -> Result<T, GatewayError>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = request_body.ok_or_else(|| bad_request_error("request body is required"))?;
    serde_json::from_slice(raw)
        .map_err(|err| bad_request_error(format!("request body must be valid JSON: {err}")))
}

fn parse_json_value_body(request_body: Option<&Bytes>) -> Result<Value, GatewayError> {
    parse_json_body::<Value>(request_body)
}

fn header_map_from_value(value: &Value) -> Result<HeaderMap, GatewayError> {
    let Some(object) = value.as_object() else {
        return Err(bad_request_error("headers must be a JSON object"));
    };
    let mut headers = HeaderMap::new();
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(bad_request_error(format!(
                "header {name} must have a string value"
            )));
        };
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| bad_request_error(format!("header {name} has invalid name")))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|_| bad_request_error(format!("header {name} has invalid value")))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

fn header_map_payload(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn patch_summary(plan: &MutationPlan) -> RoutingPatchSummary {
    RoutingPatchSummary {
        body_paths: plan
            .body_patch
            .iter()
            .map(|operation| operation.path().to_string())
            .collect(),
        header_names: plan
            .header_patch
            .iter()
            .map(|operation| match operation {
                RoutingHeaderPatch::Set { name, .. } | RoutingHeaderPatch::Remove { name } => {
                    name.clone()
                }
            })
            .collect(),
        failed_action: None,
    }
}

fn routing_group_payload(group: &StoredRoutingGroup) -> Value {
    json!({
        "id": group.id,
        "name": group.name,
        "description": group.description,
        "enabled": group.enabled,
        "is_system_default": group.is_system_default,
        "sort_order": group.sort_order,
        "config_json": group.config_json,
        "version": group.version,
        "created_at": group.created_at,
        "updated_at": group.updated_at,
        "published_at": group.published_at,
    })
}

fn routing_group_binding_payload(binding: &StoredRoutingGroupBinding) -> Value {
    json!({
        "id": binding.id,
        "group_id": binding.group_id,
        "subject_type": binding.subject_type,
        "subject_id": binding.subject_id,
        "is_default": binding.is_default,
        "allow_explicit_select": binding.allow_explicit_select,
        "created_at": binding.created_at,
        "updated_at": binding.updated_at,
    })
}

fn routing_group_version_payload(version: &StoredRoutingGroupVersion) -> Value {
    json!({
        "id": version.id,
        "group_id": version.group_id,
        "version": version.version,
        "config_json": version.config_json,
        "created_at": version.created_at,
        "created_by": version.created_by,
    })
}

fn normalized_admin_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn routing_group_path_parts(path: &str) -> Option<(String, Option<String>)> {
    let suffix = path.strip_prefix(&(ROUTING_GROUPS_ROOT.to_string() + "/"))?;
    let mut parts = suffix.split('/');
    let group_id = parts.next()?.trim();
    if group_id.is_empty() {
        return None;
    }
    let suffix = parts.next().map(str::to_string);
    if parts.next().is_some() {
        return None;
    }
    Some((group_id.to_string(), suffix))
}

fn routing_binding_id_from_path(path: &str) -> Option<String> {
    let suffix = path.strip_prefix(&(ROUTING_BINDINGS_ROOT.to_string() + "/"))?;
    if suffix.trim().is_empty() || suffix.contains('/') {
        return None;
    }
    Some(suffix.to_string())
}

fn routing_subject_from_value(value: &Value) -> Result<RoutingGroupBindingSubject, GatewayError> {
    let Some(value) = value.as_str() else {
        return Err(bad_request_error("subject_type must be a string"));
    };
    routing_subject_from_str(value)
}

fn routing_subject_from_str(value: &str) -> Result<RoutingGroupBindingSubject, GatewayError> {
    match value.trim() {
        "user" => Ok(RoutingGroupBindingSubject::User),
        "api_key" => Ok(RoutingGroupBindingSubject::ApiKey),
        "user_group" => Ok(RoutingGroupBindingSubject::UserGroup),
        other => Err(bad_request_error(format!(
            "unsupported subject_type: {other}"
        ))),
    }
}

fn required_string(value: &Value, field: &str) -> Result<String, GatewayError> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| bad_request_error(format!("{field} must be a non-empty string")))
}

fn optional_string(value: &Value, field: &str) -> Result<Option<String>, GatewayError> {
    if value.is_null() {
        return Ok(None);
    }
    required_string(value, field).map(Some)
}

fn required_bool(value: &Value, field: &str) -> Result<bool, GatewayError> {
    value
        .as_bool()
        .ok_or_else(|| bad_request_error(format!("{field} must be a boolean")))
}

fn required_i64(value: &Value, field: &str) -> Result<i64, GatewayError> {
    value
        .as_i64()
        .ok_or_else(|| bad_request_error(format!("{field} must be an integer")))
}

fn optional_i64(value: &Value, field: &str) -> Result<Option<i64>, GatewayError> {
    if value.is_null() {
        return Ok(None);
    }
    required_i64(value, field).map(Some)
}

fn default_true() -> bool {
    true
}

fn default_api_format() -> String {
    "openai:chat".to_string()
}

fn bad_request_error(detail: impl Into<String>) -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::BAD_REQUEST,
        message: detail.into(),
    }
}

fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": "routing profile data backend is unavailable" })),
    )
        .into_response()
}
